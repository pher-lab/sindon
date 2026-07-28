//! Widget tree — manages the widget hierarchy and coordinates layout/paint/events.

use std::collections::HashMap;
use std::path::Path;

use crate::accessibility::{
    A11Y_WINDOW_ROOT, AccessEntry, AccessSnapshot, AccessTarget, access_target, push_child_entries,
};
use crate::container::Container;
use crate::event::{
    EventContext, EventResult, Key, MouseButton, NamedKey, TreeCommand, WidgetEvent,
};
use crate::focus::{FocusDirection, FocusManager, FocusReason};
use crate::layer::{HAlign, LayerAnchor, LayerEntry, LayerOptions, Placement, VAlign};
use crate::paint::PaintContext;
use crate::reactive_children::ReactiveChildren;
use crate::scroll_view::ScrollView;
use crate::shortcut::ShortcutRouter;
use crate::virtual_list::VirtualList;
use crate::widget::{MeasureContext, Widget};
use sindon_core::{AccessAction, AccessNode, AccessRole, Point, Rect, SecurityLevel, Size, Theme};
use sindon_layout::{FlexStyle, LayoutEngine, LayoutNodeId};
use sindon_text::TextEngine;

/// Slack allowed when skipping the `paint` of a widget scrolled outside the
/// active clip (see [`WidgetTree::clipped_away`]).
///
/// Sized to swallow everything a stock widget draws beyond its own layout
/// rect: a focus ring reaches `ring_offset + ring_width` (4 px in both built-in
/// themes) and the deepest `Container::elevation` shadow reaches `offset_y +
/// blur` = 36 px. The contract this sets for widget authors: **a widget that
/// paints more than this far outside its layout rect must not count on running
/// while it is scrolled out of a clipping ancestor.** Nothing in the framework
/// comes close today; a hypothetical wide-halo widget would need to declare its
/// overflow rather than rely on the constant being generous.
const PAINT_CULL_MARGIN: f32 = 64.0;

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
    /// Whether this widget's style carries viewport-relative dimensions
    /// (`vh`/`vw`). Such nodes must be re-resolved on a window resize (their
    /// pixel extent depends on the viewport), so `refresh_styles` re-installs
    /// them when the viewport changes. `false` for the overwhelming majority
    /// of nodes, which keep their install-time style untouched.
    has_viewport_dims: bool,
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
    /// Last pointer position seen by [`Self::dispatch_event`], in viewport
    /// coordinates. The hover chain is otherwise only ever recomputed from a
    /// live `MouseMove`; this remembers where the cursor is so
    /// [`Self::resync_hover`] can re-resolve it after the tree changes
    /// underneath a stationary cursor. `None` until the first pointer event.
    last_pointer_pos: Option<Point>,
    /// Set when a layer pops, [`Self::remove`] tombstones the hovered widget,
    /// or a scroll view's eased offset glides content under the cursor,
    /// consumed by [`Self::resync_hover`] on the next frame. All three can
    /// leave a *different* widget sitting directly under a cursor that never
    /// moves afterwards — dismissing a menu by clicking a button behind it, a
    /// button that rebuilds its own list and is replaced in place, or a wheel
    /// scroll that slides a fresh row under the pointer — and without a resync
    /// that widget stays un-hovered until the user jiggles the mouse.
    hover_dirty: bool,
    /// Widget that has captured the pointer (e.g. an `Input` mid drag-select).
    /// While `Some`, `MouseMove` / `MouseUp` are routed directly to it,
    /// bypassing hit-testing, so the drag keeps extending past its rect. Set
    /// when a handler calls [`EventContext::capture_pointer`], cleared on
    /// [`EventContext::release_pointer`] or when the widget is removed.
    pointer_capture: Option<usize>,
    /// Tree-global keyboard focus tracker. Mutated by
    /// [`Self::advance_focus`] (Tab/Shift+Tab) and invalidated by
    /// [`Self::remove`] so a removed widget never stays focused.
    focus: FocusManager,
    /// Deferred initial focus queued by [`Self::focus_initially`] /
    /// [`Self::refocus_initially`], with the reason to apply it under.
    /// Consumed by [`Self::flush_pending_focus`] — typically called once per
    /// redraw by the event loop. One-shot: the field is taken and cleared on
    /// flush, so re-arming requires another call (e.g. inside a
    /// `replace_screen` build closure).
    pending_initial_focus: Option<(usize, FocusReason)>,
    /// The `:focus-visible` flag of the widget whose removal just cleared
    /// focus — stashed by [`Self::remove`], consumed by
    /// [`Self::refocus_initially`] so a rebuild can hand focus back exactly
    /// as it was. `None` means "no focus was lost", which is what makes
    /// `refocus_initially` a no-op rather than a focus thief.
    ///
    /// Frame-scoped: [`Self::flush_pending_focus`] drops it unconditionally,
    /// so an unconsumed stash can never be picked up by a later, unrelated
    /// rebuild.
    dropped_focus_visible: Option<bool>,
    /// Widget that focus moved to for a reason that should bring it on screen
    /// ([`FocusReason::scrolls_into_view`]), consumed by
    /// [`Self::reveal_pending_focus`] at the end of the next layout pass.
    ///
    /// Deferred rather than done at the focus call because the reveal is pure
    /// rect arithmetic and focus can be applied before the tree has ever been
    /// laid out — `focus_initially` on a freshly built screen runs *before* the
    /// first `compute_layout`, where every rect still reads zero.
    ///
    /// Assigned (not or-ed) on every focus change, so a later focus that must
    /// not scroll — a click landing elsewhere — cancels an earlier request
    /// instead of leaving it to fire against the wrong widget.
    pending_reveal: Option<usize>,
    /// Active overlay layers (modals, dropdowns, context menus), painted
    /// in push order over the main root. Topmost layer (last push) gets
    /// event priority. While `layers` is non-empty, the main tree
    /// receives no pointer or keyboard input — see [`Self::dispatch_event`].
    layers: Vec<LayerEntry>,
    /// Viewport size from the last layout pass. Held so paint and event
    /// dispatch can place layer roots relative to it without threading a
    /// size through every call site. `(0, 0)` until the first layout.
    viewport: (f32, f32),
    /// Viewport size at the last `refresh_styles` pass. When it differs from
    /// [`Self::viewport`], nodes carrying viewport-relative dimensions
    /// (`vh`/`vw`) are re-resolved; otherwise their install-time pixel style
    /// still holds and is left alone. `(0, 0)` forces a resolve on the first
    /// layout.
    last_resolved_viewport: (f32, f32),
    /// App-level keyboard shortcut registry. Consulted at the head of
    /// [`Self::dispatch_event`] for every `KeyDown`; populated via
    /// `AppScope::on_shortcut` after the build closure returns. See
    /// [`crate::shortcut`].
    shortcuts: ShortcutRouter,
    /// Screen-scoped handler for OS file drops (drag-and-drop from the
    /// desktop / file manager). Registered via [`Self::on_file_drop`] and
    /// invoked by [`Self::dispatch_file_drop`] when the event loop
    /// receives a `DroppedFile`. Cleared on every `replace_screen`
    /// transition so a handler installed by one screen never fires after
    /// that screen is torn down — matching the per-screen lifetime of the
    /// signals such a handler typically captures.
    ///
    /// Deliberately a window-level hook rather than a position-routed
    /// per-widget callback: winit 0.30 carries no drop coordinates on
    /// `DroppedFile` and suppresses cursor-move events during an OS drag,
    /// so there is no reliable way to hit-test the drop against the widget
    /// under the cursor (notably on Windows).
    file_drop_handler: Option<FileDropHandler>,
    /// Screen-scoped handler for an image pasted from the system clipboard
    /// (Ctrl/Cmd+V whose content is an image, not text). Registered via
    /// [`Self::on_image_paste`] and invoked by [`Self::dispatch_image_paste`]
    /// when the event loop finds image bytes on the clipboard. Like
    /// [`Self::file_drop_handler`] it is a window-level hook (not routed to
    /// the widget under the cursor) and cleared on every `replace_screen`
    /// transition, matching the per-screen lifetime of the signals such a
    /// handler captures.
    image_paste_handler: Option<ImagePasteHandler>,
    /// Set when [`Self::replace_root`] swaps the root subtree, consumed by the
    /// event loop via [`Self::take_root_replaced`] before the next layout. Lets
    /// the loop drop the text engine's shape cache on a screen swap so glyph
    /// geometry derived from one screen's text — the user's plaintext, for a
    /// notes app — does not outlive the screen that produced it.
    root_replaced: bool,
}

/// Handler for an OS file drop onto the window — receives the dropped
/// file's path and the event context (so it can enqueue tree commands,
/// exactly like a widget event handler). Boxed as a type alias to keep
/// the [`WidgetTree`] field and [`WidgetTree::on_file_drop`] signature
/// clear of `clippy::type_complexity`. See [`WidgetTree::on_file_drop`].
type FileDropHandler = Box<dyn FnMut(&Path, &mut EventContext)>;

/// Handler for a clipboard image paste — receives the pasted image as
/// encoded PNG bytes and the event context (so it can enqueue tree
/// commands, like any widget event handler). Boxed as a type alias to keep
/// the [`WidgetTree`] field and [`WidgetTree::on_image_paste`] signature
/// clear of `clippy::type_complexity`. See [`WidgetTree::on_image_paste`].
type ImagePasteHandler = Box<dyn FnMut(&[u8], &mut EventContext)>;

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
///   trigger rect (or above, per [`Placement`]). Horizontal placement
///   follows [`HAlign`] — left edges by default, right edges for
///   [`HAlign::End`] (CSS `right-0`), or centered — then clamps so the layer
///   stays inside the viewport. [`Placement::Auto`] flips above when below
///   would overflow the viewport bottom *and* above would fit; otherwise it
///   falls back to below and the overflow is clipped.
/// - [`LayerAnchor::Viewport`]: pins the layer to a fixed viewport
///   corner/edge (per [`HAlign`]/[`VAlign`]) plus a pixel offset, clamped on
///   both axes.
fn place_layer(anchor: LayerAnchor, size: Size, viewport: (f32, f32)) -> (f32, f32) {
    let (vw, vh) = viewport;
    match anchor {
        LayerAnchor::ViewportCenter => (
            ((vw - size.width) * 0.5).max(0.0),
            ((vh - size.height) * 0.5).max(0.0),
        ),
        LayerAnchor::AnchorRect {
            rect,
            prefer,
            align,
        } => {
            // Horizontal: align the popover edge to the trigger per `align`,
            // then clamp so it does not run off either side. `max(0)` after
            // `min(...)` handles the degenerate case where the popover is
            // wider than the viewport — pin to the left and let the right
            // edge clip.
            let x = match align {
                HAlign::Start => rect.origin.x,
                HAlign::Center => rect.origin.x + (rect.size.width - size.width) * 0.5,
                HAlign::End => rect.right() - size.width,
            };
            let x = x.min(vw - size.width).max(0.0);

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
        LayerAnchor::Viewport { h, v, offset } => {
            // Resolve the fixed viewport corner/edge, apply the pixel nudge,
            // then clamp on both axes so the layer stays on screen.
            let x = match h {
                HAlign::Start => 0.0,
                HAlign::Center => (vw - size.width) * 0.5,
                HAlign::End => vw - size.width,
            };
            let y = match v {
                VAlign::Start => 0.0,
                VAlign::Center => (vh - size.height) * 0.5,
                VAlign::End => vh - size.height,
            };
            let x = (x + offset.0).min(vw - size.width).max(0.0);
            let y = (y + offset.1).min(vh - size.height).max(0.0);
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
            last_pointer_pos: None,
            hover_dirty: false,
            pointer_capture: None,
            focus: FocusManager::new(),
            pending_initial_focus: None,
            dropped_focus_visible: None,
            pending_reveal: None,
            layers: Vec::new(),
            root_replaced: false,
            viewport: (0.0, 0.0),
            last_resolved_viewport: (0.0, 0.0),
            shortcuts: ShortcutRouter::new(),
            file_drop_handler: None,
            image_paste_handler: None,
        }
    }

    /// Mutable access to the app-level shortcut registry.
    ///
    /// Used by `sindon_app::AppScope` to drain queued shortcut
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
        self.pending_initial_focus = Some((idx, FocusReason::Programmatic));
    }

    /// Hand focus back to a widget that a rebuild just replaced, preserving
    /// the focus ring exactly as it was.
    ///
    /// [`Self::rebuild_children`](crate::event::EventContext::rebuild_children)
    /// tombstones every child before running the builder, so a row's own
    /// button that rebuilds its list — a pin toggle that re-sorts, a filter
    /// chip that re-queries — activates itself out of existence, and
    /// [`Self::remove`] drops focus with it. The replacement is a fresh index,
    /// so the tree cannot know it is "the same" widget: only the app holds
    /// that identity mapping. Call this from inside the build closure once the
    /// replacement is in the tree to close the loop.
    ///
    /// Prefer this over [`Self::focus_initially`] whenever focus is being
    /// *restored* rather than moved. `focus_initially` is programmatic, so it
    /// always shows a ring; using it here would paint one on a widget the user
    /// reached with the mouse. This carries the previous ring state instead,
    /// which makes the rebuild invisible to the `:focus-visible` heuristic —
    /// a Space-activated pin keeps its ring, a clicked one stays ringless.
    ///
    /// A no-op unless a removal cleared focus earlier in this same frame, so
    /// arming it for a row that never had focus cannot steal focus from
    /// elsewhere. Like `focus_initially` it is one-shot and overwrites any
    /// prior pending target.
    pub fn refocus_initially(&mut self, idx: usize) {
        // No focus was lost → nothing to restore. Bailing (rather than
        // focusing anyway) is what lets a builder arm this unconditionally
        // for its "should be refocused" row without a `focused()` check.
        let Some(visible) = self.dropped_focus_visible.take() else {
            return;
        };
        self.pending_initial_focus = Some((idx, FocusReason::Restored { visible }));
    }

    /// Apply any pending initial focus from [`Self::focus_initially`] or
    /// [`Self::refocus_initially`].
    ///
    /// Called by the event loop at the top of each redraw. Cheap when
    /// nothing is pending (single field check). When a target is pending,
    /// dispatches `FocusLost`/`FocusGained` through [`Self::focus`], and
    /// drains any commands those handlers enqueue so the focus change
    /// settles before paint.
    pub fn flush_pending_focus(&mut self, event_ctx: &mut EventContext) {
        // End of the window in which a removal's ring state can be claimed.
        // Every rebuild is followed by a redraw, so a builder that wanted to
        // restore focus has already called `refocus_initially` (which takes
        // the stash) by now; anything left is from a removal nobody restored.
        // Dropping it here — before the early return below, so it happens on
        // every frame — keeps a stale flag from being handed to an unrelated
        // rebuild a hundred frames later.
        self.dropped_focus_visible = None;

        let Some((target, reason)) = self.pending_initial_focus.take() else {
            return;
        };
        // Race: target tombstoned between the arming call and flush. Skip
        // the focus call entirely — `WidgetTree::focus` would still update
        // the FocusManager pointer to a tombstoned slot otherwise (it only
        // guards the FocusGained dispatch, not the pointer set). One-shot
        // semantics still hold because `take()` cleared the pending field.
        if !self.contains(target) {
            return;
        }
        self.focus_with_reason(Some(target), reason, event_ctx);
        self.drain_commands(event_ctx);
    }

    /// Apply commands enqueued on `event_ctx` from outside an event
    /// dispatch.
    ///
    /// Event handlers drain automatically at the end of
    /// [`Self::dispatch_event`], and initial focus drains inside
    /// [`Self::flush_pending_focus`]. The per-frame tick hook
    /// (`AppScope::on_frame`) runs outside both, so the event loop calls
    /// this right after firing the hook to apply anything it requested —
    /// most importantly a `replace_screen` for an auto-lock. Loops until
    /// the queue settles, since applied commands' handlers may enqueue
    /// more (same drain semantics as dispatch).
    pub fn apply_pending_commands(&mut self, event_ctx: &mut EventContext) {
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
        // Boot / test push: no event handler owns it, so no opener.
        self.push_layer_boxed(options, Box::new(widget), None)
    }

    /// Remove the top layer (last pushed), tombstoning its entire subtree.
    ///
    /// No-op when no layer is active. Returns the removed layer's root
    /// index for callers that want to assert or log the dismiss.
    pub fn pop_top_layer(&mut self) -> Option<usize> {
        let entry = self.layers.pop()?;
        let root = entry.root;
        let had_focus = self.focus.focused();
        self.remove(root);
        self.hover_dirty = true;
        self.return_focus_to_opener(&entry, had_focus);
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
        let entry = self.layers.remove(pos);
        let had_focus = self.focus.focused();
        self.remove(root);
        self.hover_dirty = true;
        self.return_focus_to_opener(&entry, had_focus);
        true
    }

    /// Hand focus back to the widget that opened a just-popped layer, when the
    /// pop is what took focus away.
    ///
    /// A dialog's own fields go down with its subtree, and [`Self::remove`]
    /// drops focus with them — leaving focus nowhere, so the next Tab restarts
    /// at the top of the window and the user loses their place. The trigger is
    /// where they logically are once the layer is gone, and the tree already
    /// knows which widget that is: the layer stamped its opener at push.
    ///
    /// `had_focus` is the focus from *before* the removal, so this only fires
    /// when this pop is what cleared it. That matters twice over: a layer whose
    /// content never held focus (a menu the user opened and dismissed without
    /// ever stepping into — focus is still out on the trigger, or wherever a
    /// right-click left it) must leave focus where the user actually left it,
    /// and the ring flag stashed by `remove` is only ours to claim when our own
    /// removal stashed it.
    ///
    /// Deferred through the same one-shot pending slot as
    /// [`Self::focus_initially`], because the pop entrypoints are public and
    /// have no [`EventContext`] to dispatch `FocusGained` with.
    fn return_focus_to_opener(&mut self, entry: &LayerEntry, had_focus: Option<usize>) {
        // Focus survived the pop (it was outside the layer) — nothing to hand
        // back, and stealing it to the trigger would be a bug of its own.
        if had_focus.is_none() || self.focus.focused().is_some() {
            return;
        }
        let Some(visible) = self.dropped_focus_visible.take() else {
            return;
        };
        // No opener: pushed at boot or by a test, so there is nothing to
        // return to. Focus stays cleared.
        let Some(opener) = entry.opener else { return };
        // The trigger may not have outlived its own layer — a row's menu whose
        // action rebuilt that very row. Indices are never recycled (`remove`
        // tombstones the slot for good), so a live index is always the same
        // widget it was at push, never an unrelated one that took its place.
        if !self.contains(opener) {
            return;
        }
        // A pure-pointer trigger (`Container::on_press`, by design not a tab
        // stop — see `Button::on_click_rect` for the a11y-complete counterpart)
        // is not somewhere keyboard focus can live: it paints no ring and drops
        // straight out of the tab order, so parking focus there would only look
        // restored. The *place* still is somewhere the user is, though: anchor
        // sequential navigation to it, exactly as a click on the trigger would
        // have, so the next Tab resumes beside it instead of at the top.
        if !self.widget(opener).focusable() {
            self.focus.set_nav_start(Some(opener));
            return;
        }
        self.pending_initial_focus = Some((opener, FocusReason::Returned { visible }));
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

    /// The topmost layer that participates in event routing, skipping any
    /// non-interactive (click-through) layers stacked above it. `None` when
    /// no interactive layer is active — in which case events fall through to
    /// the main tree even if a tooltip overlay is painted on top.
    ///
    /// This is the layer that owns pointer/keyboard input, traps Tab, and
    /// is the target of Escape / outside-click dismiss — as opposed to
    /// [`Self::top_layer_root`], which is the literal topmost layer (used
    /// by callers that mean "whatever paints last").
    fn topmost_interactive_layer(&self) -> Option<&LayerEntry> {
        self.layers.iter().rev().find(|l| l.options.interactive)
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

        // Clear hover if it landed on something we're about to remove, and
        // ask for a resync: a rebuild typically drops a fresh node right back
        // under the stationary cursor (knot's pin star replaces its own row),
        // and hover is only otherwise recomputed from a live `MouseMove`, so
        // the replacement would sit un-hovered until the user moved the mouse.
        // No `MouseLeave` here — the widget is being dropped, not left.
        let hover_dropped = self.hovered.is_some_and(|h| to_remove.contains(&h));
        if hover_dropped {
            self.hovered = None;
            self.hover_dirty = true;
        }

        // Clear focus if it landed on something we're about to remove —
        // otherwise a later `advance_focus` would find the current index
        // not in the fresh tab order and jump to first/last, which looks
        // fine but masks the bug of the focus pointer dangling on a
        // tombstoned slot.
        //
        // Stash the ring state on the way out. A rebuild often replaces the
        // focused widget with an equivalent one (see `refocus_initially`);
        // the flag has to be captured here because `focus.set(None)` forces
        // it false, so by the time the builder runs it is gone.
        if let Some(f) = self.focus.focused()
            && to_remove.contains(&f)
        {
            self.dropped_focus_visible = Some(self.focus.visible());
            self.focus.set(None);
        }

        // Drop a pointer capture held by anything we're about to remove, so a
        // tombstoned index can't keep receiving routed drag events.
        if let Some(c) = self.pointer_capture
            && to_remove.contains(&c)
        {
            self.pointer_capture = None;
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

        // A teardown that swallows the hovered node produces no `MouseLeave`
        // (see above), so a tooltip armed or shown for it would be stranded:
        // its trigger is gone and nothing will ever dismiss it. Cancel here,
        // where we already know the hovered node went away — that covers both
        // a rebuilt list and a whole-screen swap, and is why apps have no
        // tooltip state to reset. Runs after the drop loop so the pop below
        // sees a settled tree.
        if hover_dropped && let Some(tip) = crate::tooltip::cancel() {
            self.pop_layer(tip);
        }
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
        let style = widget.style();
        let has_viewport_dims = style.has_viewport_dims();
        let style = Self::effective_style(style, false, initial_visible, self.viewport);
        let layout_node = self.layout.add_leaf(style);
        let idx = self.nodes.len();
        self.nodes.push(Some(WidgetNode {
            widget,
            layout_node,
            children: Vec::new(),
            parent: None,
            effective_security,
            last_applied_visible: initial_visible,
            has_viewport_dims,
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
        // Direct children of a `ScrollView` must keep their natural intrinsic
        // size — Taffy's default `flex-shrink: 1` on a fixed-height column
        // would otherwise squash them to fit the viewport, defeating the whole
        // point of a scrollable container (this is what `overflow: scroll`
        // does for free in CSS).
        let parent_is_scroll = Self::is_scroll_view_idx(&self.nodes, parent);
        let style = widget.style();
        let has_viewport_dims = style.has_viewport_dims();
        let style = Self::effective_style(style, parent_is_scroll, initial_visible, self.viewport);
        let layout_node = self.layout.add_leaf(style);
        let idx = self.nodes.len();
        self.nodes.push(Some(WidgetNode {
            widget,
            layout_node,
            children: Vec::new(),
            parent: Some(parent),
            effective_security,
            last_applied_visible: initial_visible,
            has_viewport_dims,
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
        // Signal the event loop to drop the shape cache before the next layout:
        // the old screen's text (plaintext, for a notes app) is gone, so its
        // cached glyph geometry should not linger.
        self.root_replaced = true;
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
        opener: Option<usize>,
    ) -> usize {
        let effective_security = widget.security_level();
        let initial_visible = widget.visible();
        let style = widget.style();
        let has_viewport_dims = style.has_viewport_dims();
        let style = Self::effective_style(style, false, initial_visible, self.viewport);
        let layout_node = self.layout.add_leaf(style);
        let idx = self.nodes.len();
        self.nodes.push(Some(WidgetNode {
            widget,
            layout_node,
            children: Vec::new(),
            parent: None,
            effective_security,
            last_applied_visible: initial_visible,
            has_viewport_dims,
        }));
        let interactive = options.interactive;
        self.layers.push(LayerEntry {
            root: idx,
            options,
            offset: (0.0, 0.0),
            measured_size: Size::ZERO,
            opener,
        });
        // An interactive layer takes input priority, so the cursor is no
        // longer "over" any main-tree widget — drop the hover pointer so a
        // stale MouseLeave doesn't fire at the wrong time when the layer
        // pops. On the event path, `apply_commands` has already emitted a
        // real MouseLeave to this chain (resetting the trigger's hover
        // visual — G15) just before calling us, so this is a no-op null
        // there; on the boot/test path there is no live hover to leave. A
        // non-interactive (click-through) tooltip layer leaves input with
        // the main tree, so its hover chain must persist: clearing it would
        // make the trigger see a spurious re-enter on the next move and
        // re-open the very tip it just opened.
        if interactive {
            self.hovered = None;
        }
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
                    // A screen swap also tears down any open layers (modals,
                    // dropdowns, context menus) — a layer belongs to the screen
                    // that opened it. Without this, a `replace_screen` fired
                    // while a modal is up (an idle auto-lock, or a restore that
                    // returns to the lock screen from inside its confirm dialog)
                    // would leave the stale modal hovering over the new screen.
                    // Snapshot the roots first since `remove` mutates `layers`.
                    let layer_roots: Vec<usize> = self.layers.iter().map(|l| l.root).collect();
                    for root in layer_roots {
                        self.remove(root);
                    }
                    self.layers.clear();
                    // A screen swap tears down the screen that registered
                    // any file-drop or image-paste handler; clear them so a
                    // stale handler (capturing the old screen's signals)
                    // never fires on the new screen. The replacement
                    // re-registers in its build closure if it wants them.
                    self.file_drop_handler = None;
                    self.image_paste_handler = None;
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
                    if let Some(idx) = target
                        && !self.contains(idx)
                    {
                        continue;
                    }
                    self.focus(target, event_ctx);
                }
                TreeCommand::PushLayer {
                    options,
                    root_widget,
                    populate,
                    opener,
                } => {
                    // An interactive layer takes input priority: the cursor is
                    // no longer "over" any main-tree widget. Emit MouseLeave to
                    // the live hover chain *before* the push nulls the pointer,
                    // so a hoverable trigger (gear / ⋮ menu button) that opened
                    // this layer resets its hover visual instead of sticking
                    // highlighted after the layer closes (G15). This must run
                    // here rather than in `push_layer_boxed`, because emitting
                    // leaves needs an `event_ctx` and only the drain/event path
                    // carries one; the boot/test push has no live hover anyway.
                    if options.interactive {
                        self.clear_hover(event_ctx);
                    }
                    let layer_root = self.push_layer_boxed(options, root_widget, opener);
                    populate(self, layer_root);
                }
                TreeCommand::PopLayer { root } => {
                    self.pop_layer(root);
                }
                TreeCommand::PopTopLayer => {
                    self.pop_top_layer();
                }
                TreeCommand::AdvanceFocus { dir } => {
                    self.advance_focus(dir, event_ctx);
                }
                TreeCommand::Reveal { idx } => {
                    // Assigned, not or-ed — the same rule the focus path uses
                    // (see `focus_with_reason`), which is what lets a reveal
                    // queued after a focus supersede the one that focus armed.
                    if self.contains(idx) {
                        self.pending_reveal = Some(idx);
                    }
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
        self.sync_reactive_children();
        self.sync_virtual_lists();
        self.refresh_styles();
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
        self.reveal_pending_focus();
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

    /// Resolve a widget's builder style into the Taffy-ready form installed on
    /// its layout node, applying every context-dependent adjustment in one
    /// place so the add path and [`Self::refresh_styles`] stay consistent:
    ///
    /// - **scroll-shrink**: direct children of a `ScrollView` get
    ///   `flex-shrink: 0` so a fixed-height column can't squash them (see
    ///   [`Self::add_child_boxed`]).
    /// - **viewport dims**: any `vh`/`vw` dimension is baked to pixels against
    ///   the current viewport (see [`FlexStyle::resolve_viewport`]).
    /// - **visibility**: a hidden widget collapses to `display: none`.
    fn effective_style(
        style: FlexStyle,
        parent_is_scroll: bool,
        visible: bool,
        viewport: (f32, f32),
    ) -> FlexStyle {
        let mut style = style;
        if parent_is_scroll {
            style = style.shrink(0.0);
        }
        style = style.resolve_viewport(viewport.0, viewport.1);
        if !visible {
            style = style.display_none();
        }
        style
    }

    /// Re-apply Taffy styles for nodes whose effective style may have changed
    /// since the last layout pass — either their `visible()` flipped, or they
    /// carry viewport-relative dimensions (`vh`/`vw`) and the viewport
    /// resized. A widget that went hidden gets `display: none`; one that came
    /// back visible gets its real style re-installed; a `vh`/`vw` node gets
    /// its pixel extent recomputed for the new viewport. Nodes with neither
    /// trigger are left alone (their install-time style still applies) to
    /// avoid redundant `set_style` churn every frame.
    ///
    /// All re-installs go through [`Self::effective_style`], so the
    /// `ScrollView` `flex-shrink: 0` override and viewport resolution are
    /// re-applied together and never clobber one another.
    fn refresh_styles(&mut self) {
        let viewport = self.viewport;
        let viewport_changed = viewport != self.last_resolved_viewport;
        // Snapshot dirty indices first so we can read `is_scroll_view_idx`
        // (borrows `self.nodes`) without overlapping the per-node mut borrow
        // we need for `set_style`.
        let dirty: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let node = slot.as_ref()?;
                let vis_flip = node.widget.visible() != node.last_applied_visible;
                let vp_reflow = node.has_viewport_dims && viewport_changed;
                // A dynamic-style widget (a resizable split pane) re-reads its
                // style every frame so a drag-driven flex-grow reaches layout.
                let dyn_style = node.widget.style_is_dynamic();
                (vis_flip || vp_reflow || dyn_style).then_some(i)
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
            let style = Self::effective_style(node.widget.style(), parent_is_scroll, vis, viewport);
            let layout_node = node.layout_node;
            node.last_applied_visible = vis;
            self.layout.set_style(layout_node, style);
        }
        self.last_resolved_viewport = viewport;
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
        self.sync_reactive_children();
        self.sync_virtual_lists();
        self.refresh_styles();

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
        self.reveal_pending_focus();
    }

    /// Walk every live widget; for each [`ReactiveChildren`], compare its
    /// current version token against the one its children were last built at,
    /// and rebuild the subtree when they differ.
    ///
    /// Called at the *top* of both `compute_layout` and
    /// `compute_layout_with_measure` so a rebuild's fresh children get Taffy
    /// nodes and are measured/laid out in the same pass (the scroll auto-height
    /// pass at the end then sees them). When nothing changed the cost is one
    /// `version()` call (a cheap compare) per node.
    ///
    /// The rebuild mirrors the `TreeCommand::RebuildChildren` arm: tombstone
    /// every current child of the node, then run the builder against the stable
    /// parent index. Interior mutability on the widget (`RefCell`/`Cell`) lets
    /// us take the builder out and update the version through the `&self` the
    /// downcast yields, so it doesn't fight the `&mut self` that `remove` and
    /// the builder need.
    fn sync_reactive_children(&mut self) {
        let candidates: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let node = slot.as_ref()?;
                let w: &dyn Widget = node.widget.as_ref();
                (w as &dyn std::any::Any)
                    .downcast_ref::<ReactiveChildren>()
                    .map(|_| i)
            })
            .collect();

        for idx in candidates {
            // Decide + take the builder under a short borrow, then drop it
            // before touching `&mut self`.
            let builder = {
                let Some(node) = self.node(idx) else { continue };
                let w: &dyn Widget = node.widget.as_ref();
                let Some(rc) = (w as &dyn std::any::Any).downcast_ref::<ReactiveChildren>() else {
                    continue;
                };
                let v = rc.version();
                if rc.last_version() == Some(v) {
                    continue; // unchanged — nothing to rebuild
                }
                rc.set_last_version(v);
                match rc.take_builder() {
                    Some(b) => b,
                    None => continue, // source-less node, or reentrant take
                }
            };

            // Tombstone current children, then repopulate from the builder.
            let children: Vec<usize> = self
                .node(idx)
                .map(|n| n.children.clone())
                .unwrap_or_default();
            for c in children {
                self.remove(c);
            }
            let mut builder = builder;
            builder(self, idx);

            // Put the builder back for the next change.
            if let Some(node) = self.node(idx) {
                let w: &dyn Widget = node.widget.as_ref();
                if let Some(rc) = (w as &dyn std::any::Any).downcast_ref::<ReactiveChildren>() {
                    rc.restore_builder(builder);
                }
            }
        }
    }

    /// Windowing pass for [`VirtualList`](crate::VirtualList) nodes. Runs right
    /// after [`Self::sync_reactive_children`] (before layout): for each virtual
    /// list, read the eased scroll offset + viewport height from the enclosing
    /// `ScrollView`, pin that scroll view's content height to the full logical
    /// extent (item count × row height), compute the visible integer row range,
    /// and rebuild the row subtree only when the current visible range has left
    /// the previously-built (overscanned) window — or the app bumped the content
    /// version, or the item count changed.
    ///
    /// The scroll view keeps owning scroll/ease/clip/scrollbar; this pass only
    /// decides which rows exist and inserts a leading spacer so the first
    /// materialized row sits at its true logical `y`.
    fn sync_virtual_lists(&mut self) {
        let candidates: Vec<usize> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                let node = slot.as_ref()?;
                let w: &dyn Widget = node.widget.as_ref();
                (w as &dyn std::any::Any)
                    .downcast_ref::<VirtualList>()
                    .map(|_| i)
            })
            .collect();

        for vl_idx in candidates {
            // Fixed parameters + the app's count / version.
            let (row_h, overscan, count, data_v) = {
                let Some(node) = self.node(vl_idx) else {
                    continue;
                };
                let w: &dyn Widget = node.widget.as_ref();
                let Some(vl) = (w as &dyn std::any::Any).downcast_ref::<VirtualList>() else {
                    continue;
                };
                (
                    vl.row_height(),
                    vl.overscan_rows(),
                    vl.item_count(),
                    vl.content_version(),
                )
            };

            // Enclosing scroll view: its eased offset + viewport height (from the
            // previous layout pass) define the visible window. Missing on the
            // first pass — the fallback below then materializes a screenful.
            let sv_idx = self.enclosing_scroll_view(vl_idx);
            let (offset_y, viewport_h) = match sv_idx {
                Some(sv) => {
                    let node = self
                        .node(sv)
                        .expect("scroll-view idx from parent walk is live");
                    let off = node.widget.scroll_offset().1;
                    let vh = self.layout.layout(node.layout_node).size.height;
                    (off, vh)
                }
                None => (0.0, f32::MAX),
            };

            // Pin the scroll view's content height to the full logical extent so
            // the scrollbar + clamp span all rows though only a window exists.
            if let Some(sv) = sv_idx
                && let Some(node) = self.node_mut(sv)
            {
                let widget_any: &mut dyn std::any::Any = node.widget.as_mut();
                if let Some(scroll) = widget_any.downcast_mut::<ScrollView>() {
                    scroll.set_pinned_content_height(count as f32 * row_h);
                }
            }

            // Current visible integer row range [visible_first, visible_last).
            let vh = if viewport_h.is_finite() && viewport_h > 1.0 {
                viewport_h
            } else {
                1000.0
            };
            let visible_first = (offset_y / row_h).floor().max(0.0) as usize;
            let visible_last = (((offset_y + vh) / row_h).ceil() as usize).min(count);

            // Skip the rebuild while the last-built window still covers the
            // visible range and nothing structural changed.
            if let Some((bf, bl, bv, bc)) = {
                let node = self.node(vl_idx);
                node.and_then(|n| {
                    let w: &dyn Widget = n.widget.as_ref();
                    (w as &dyn std::any::Any)
                        .downcast_ref::<VirtualList>()
                        .and_then(|vl| vl.last_window())
                })
            } && bv == data_v
                && bc == count
                && bf <= visible_first
                && visible_last <= bl
            {
                continue;
            }

            // Rebuild: overscan the visible range so nearby scrolls stay cheap.
            let first = visible_first.saturating_sub(overscan);
            let last = (visible_last + overscan).min(count);
            let window = (first, last, data_v, count);

            let builder = {
                let Some(node) = self.node(vl_idx) else {
                    continue;
                };
                let w: &dyn Widget = node.widget.as_ref();
                let Some(vl) = (w as &dyn std::any::Any).downcast_ref::<VirtualList>() else {
                    continue;
                };
                vl.set_last_window(window);
                match vl.take_builder() {
                    Some(b) => b,
                    None => continue, // no row builder, or a reentrant take
                }
            };

            // Tombstone the current spacer + rows, then repopulate the window.
            let children: Vec<usize> = self
                .node(vl_idx)
                .map(|n| n.children.clone())
                .unwrap_or_default();
            for c in children {
                self.remove(c);
            }

            if first > 0 {
                self.add_child(
                    vl_idx,
                    Container::column()
                        .width_full()
                        .height(first as f32 * row_h),
                );
            }
            let mut builder = builder;
            for i in first..last {
                builder(self, vl_idx, i);
            }

            if let Some(node) = self.node(vl_idx) {
                let w: &dyn Widget = node.widget.as_ref();
                if let Some(vl) = (w as &dyn std::any::Any).downcast_ref::<VirtualList>() {
                    vl.restore_builder(builder);
                }
            }
        }
    }

    /// Nearest ancestor that is a [`ScrollView`], walking `parent` links. `None`
    /// if the node has no scroll-view ancestor.
    fn enclosing_scroll_view(&self, start: usize) -> Option<usize> {
        let mut idx = start;
        while let Some(node) = self.node(idx) {
            let parent = node.parent?;
            let parent_node = self.node(parent)?;
            let w: &dyn Widget = parent_node.widget.as_ref();
            if (w as &dyn std::any::Any)
                .downcast_ref::<ScrollView>()
                .is_some()
            {
                return Some(parent);
            }
            idx = parent;
        }
        None
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

        // Set once any scroll view's eased offset moved this pass, so the
        // hover hit-test is replayed below: a wheel glide slides content under
        // a stationary cursor with no `MouseMove` to refresh hover.
        let mut scroll_moved = false;

        for sv_idx in candidates {
            let Some(node) = self.node(sv_idx) else {
                continue;
            };
            let children = node.children.clone();
            let sv_layout_node = node.layout_node;
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
            // Capture the viewport height before the mutable borrow below so we
            // can re-clamp the scroll offset against the (just-updated) content
            // extent in the same pass.
            let viewport_h = self.layout.layout(sv_layout_node).size.height;

            let sv_node = self
                .node_mut(sv_idx)
                .expect("scroll-view slot vanished between read and write");
            let widget_any: &mut dyn std::any::Any = sv_node.widget.as_mut();
            if let Some(sv) = widget_any.downcast_mut::<ScrollView>() {
                let bottom_pad = sv.measured_bottom_padding();
                sv.set_measured_content_height(max_bottom + bottom_pad);
                // Content may have shrunk (e.g. switched to a shorter note); pull
                // a now-too-large offset back so the top isn't scrolled out of
                // view. Growing content leaves the offset alone.
                sv.clamp_scroll(viewport_h);
                // Poll after the clamp so a re-clamp snap counts as movement.
                scroll_moved |= sv.take_scroll_moved();
            }
        }

        // A glide (or a clamp snap) shifted content under a cursor that never
        // moved. Ask the next `resync_hover` to replay the hit-test, the same
        // way a layer pop or a self-rebuild does — hover is purely geometric,
        // so the replay re-derives it from `last_pointer_pos` alone. Deferred
        // under a pointer capture and skipped before the first pointer event,
        // both handled inside `resync_hover`.
        if scroll_moved {
            self.hover_dirty = true;
        }
    }

    /// Scroll every `ScrollView` ancestor of the pending-reveal widget so that
    /// widget sits inside their viewports — the "Tab followed the focus"
    /// behavior. A no-op unless a focus change armed a request (see
    /// [`Self::pending_reveal`]).
    ///
    /// Runs at the tail of the layout pass, after
    /// [`Self::sync_scroll_view_content_heights`], because both of its inputs
    /// are only valid there: the rects it measures come from the layout that
    /// just ran, and the clamp it relies on needs the content extents that
    /// pass publishes.
    fn reveal_pending_focus(&mut self) {
        let Some(idx) = self.pending_reveal.take() else {
            return;
        };
        // The target may have been removed between the focus and this layout.
        let Some(node) = self.node(idx) else { return };
        let target = self.layout.absolute_rect(node.layout_node);
        let mut parent = node.parent;
        // Scroll settled by the scroll ancestors already revealed. An outer
        // viewport sees the target displaced by every scroll between them, so
        // each step folds in what the previous one landed on.
        let mut inner_scroll = 0.0f32;

        while let Some(a) = parent {
            let Some(anode) = self.node(a) else { break };
            parent = anode.parent;
            if !Self::is_scroll_view_idx(&self.nodes, a) {
                continue;
            }
            let view = self.layout.absolute_rect(anode.layout_node);
            // Both rects come from the same un-scrolled layout frame, so their
            // difference is where this viewport would show the target at scroll
            // 0 — i.e. the target's position in the view's content space.
            let top = target.origin.y - inner_scroll - view.origin.y;
            let Some(anode) = self.node_mut(a) else { break };
            let widget_any: &mut dyn std::any::Any = anode.widget.as_mut();
            if let Some(sv) = widget_any.downcast_mut::<ScrollView>() {
                inner_scroll += sv.reveal_span(top, target.size.height, view.size.height);
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
        // Publish the `:focus-visible` state for this frame so the focused
        // widget knows whether to paint its ring (suppressed for pointer
        // focus, shown for keyboard / programmatic).
        ctx.set_focus_visible(self.focus.visible());
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
        // Scrolled out of view? Skip the widget's own paint. Dropping the draw
        // commands it would emit is already handled exactly by `PaintContext`
        // (they'd be scissored away); the point of stopping one level earlier
        // is the *work upstream of them* — a `TextWidget` shapes its string and
        // rasterizes every glyph before it draws anything, so a 500-block
        // markdown preview pays for the whole document on every frame to show
        // one screenful. `Input` already culled its own glyphs this way; this
        // generalizes it to any widget under any clip.
        //
        // Children are still walked: a widget's subtree is not guaranteed to
        // sit inside its box (a `ScrollView`'s content column is taller than
        // the viewport by definition), and the walk itself is cheap next to
        // the shaping it guards.
        if !self.clipped_away(layout_rect, idx, ctx) {
            node.widget.paint(layout_rect, ctx);
        }

        node.widget.paint_pre_children(layout_rect, ctx);
        for &child in &node.children {
            self.paint_node(child, ctx);
        }
        node.widget.paint_post_children(layout_rect, ctx);
    }

    /// Whether node `idx`, laid out at `layout_rect`, sits far enough outside
    /// the active clip that skipping its `paint` cannot change a pixel.
    ///
    /// Unlike the per-command test in [`PaintContext`], this one is a
    /// *heuristic*, because a widget may legitimately draw outside its own
    /// layout rect: a focus ring sits `ring_offset + ring_width` px beyond the
    /// edge, and `Container::shadow` casts a halo reaching `offset + blur` px
    /// past it. [`PAINT_CULL_MARGIN`] is sized to clear both with room to
    /// spare — see its docs for the contract a widget has to honor.
    ///
    /// The focused widget is never culled. Its `paint` is not purely visual:
    /// `Input` republishes the IME cursor area and schedules the next caret
    /// blink from there, and silently dropping that while the field is merely
    /// scrolled out of view would leave the OS candidate window anchored to a
    /// stale position.
    fn clipped_away(&self, layout_rect: Rect, idx: usize, ctx: &PaintContext) -> bool {
        let Some(clip) = ctx.current_clip() else {
            return false;
        };
        if self.focus.focused() == Some(idx) {
            return false;
        }
        let (ox, oy) = ctx.current_offset();
        let m = PAINT_CULL_MARGIN;
        let (left, top) = (layout_rect.origin.x + ox - m, layout_rect.origin.y + oy - m);
        let (right, bottom) = (layout_rect.right() + ox + m, layout_rect.bottom() + oy + m);
        right <= clip.origin.x
            || left >= clip.right()
            || bottom <= clip.origin.y
            || top >= clip.bottom()
    }

    /// Build a framework-native accessibility snapshot of the whole tree
    /// (main root + every overlay layer) for OS assistive technology.
    ///
    /// See [`crate::accessibility`]. The caller (`sindon_app`) invokes this
    /// only while an assistive technology is connected, so its cost — a tree
    /// walk plus a reactive read per node for the label / value — is paid only
    /// when a screen reader is actually listening.
    ///
    /// Every node's bounds are in viewport coordinates (a layer's anchor
    /// offset is folded in), matching what paint and hit-testing use.
    pub fn accessibility_snapshot(&self) -> AccessSnapshot {
        let mut entries: Vec<AccessEntry> = Vec::new();
        let mut root_children: Vec<u64> = Vec::new();

        // Main tree, if any and visible.
        if let Some(root) = self.root
            && self.node(root).is_some_and(|n| n.widget.visible())
        {
            root_children.push(root as u64);
            self.walk_access(root, (0.0, 0.0), None, &mut entries);
        }

        // Overlay layers. The topmost interactive layer is the modal surface;
        // each layer's rects are in its own local frame, so we fold in the
        // layer offset to land them in viewport space (as paint does).
        let modal_root = self.topmost_interactive_layer().map(|l| l.root);
        let layer_roots: Vec<(usize, (f32, f32))> =
            self.layers.iter().map(|l| (l.root, l.offset)).collect();
        for (layer_root, offset) in layer_roots {
            if self.node(layer_root).is_some_and(|n| n.widget.visible()) {
                root_children.push(layer_root as u64);
                self.walk_access(layer_root, offset, modal_root, &mut entries);
            }
        }

        // Synthetic window root bundling the main tree and all layers under a
        // single a11y root (accesskit requires exactly one).
        entries.push(AccessEntry {
            id: A11Y_WINDOW_ROOT,
            node: AccessNode::new(AccessRole::Window),
            bounds: Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1),
            children: root_children,
            modal: false,
            focusable: false,
        });

        // Focus must name a node present in the snapshot; fall back to the
        // window root if the focused widget just went invisible / away.
        //
        // A roving container (`TreeView`) keeps keyboard focus on itself and
        // moves a cursor between its rows, so it redirects a11y focus to the
        // cursor row — otherwise a screen reader, which follows this id, would
        // announce the container once and stay silent for every arrow key. The
        // delegate is honored only if it too is in the snapshot, so one left
        // stale by a rebuild degrades to focusing the container.
        let present = |id: usize| entries.iter().any(|e| e.id == id as u64);
        let focus_id = match self.focus.focused() {
            Some(idx) if present(idx) => self
                .node(idx)
                .and_then(|n| n.widget.accessibility_focus_delegate())
                .filter(|&d| present(d))
                .unwrap_or(idx) as u64,
            _ => A11Y_WINDOW_ROOT,
        };

        AccessSnapshot {
            root_id: A11Y_WINDOW_ROOT,
            focus_id,
            entries,
        }
    }

    /// Emit access entries for `idx` and its visible descendants, shifting
    /// each rect by `offset` (a layer's viewport offset; `(0, 0)` for the main
    /// tree). `modal_root`, when it matches a node, flags that node as a modal
    /// dialog surface.
    fn walk_access(
        &self,
        idx: usize,
        offset: (f32, f32),
        modal_root: Option<usize>,
        entries: &mut Vec<AccessEntry>,
    ) {
        let Some(node) = self.node(idx) else { return };
        if !node.widget.visible() {
            return;
        }
        let rect = self.layout.absolute_rect(node.layout_node);
        let bounds = Rect::new(
            rect.origin.x + offset.0,
            rect.origin.y + offset.1,
            rect.size.width,
            rect.size.height,
        );

        let mut access = node
            .widget
            .accessibility()
            .unwrap_or_else(|| AccessNode::new(AccessRole::Group));
        let modal = modal_root == Some(idx);
        if modal {
            // A layer root is a container (Group by default); present it as a
            // dialog surface so ATs announce the modal boundary.
            access.role = AccessRole::Dialog;
        }

        // Only visible children are referenced, so every child id resolves to
        // an entry we will emit (accesskit rejects dangling child refs).
        let mut children: Vec<u64> = node
            .children
            .iter()
            .filter(|&&c| self.node(c).is_some_and(|n| n.widget.visible()))
            .map(|&c| c as u64)
            .collect();

        // A composite control (Segmented / RadioGroup) paints its options
        // instead of owning child widgets, so it contributes their nodes here.
        // Its own rect is what paint sees (layer offset not yet folded in), so
        // the derived option rects get the same shift as the owner's bounds.
        let options = node.widget.accessibility_children(rect);
        if !options.is_empty() {
            push_child_entries(idx, options, offset, &mut children, entries);
        }

        entries.push(AccessEntry {
            id: idx as u64,
            node: access,
            bounds,
            children,
            modal,
            focusable: node.widget.focusable(),
        });

        let child_indices: Vec<usize> = node.children.clone();
        for c in child_indices {
            self.walk_access(c, offset, modal_root, entries);
        }
    }

    /// Perform an action an assistive technology requested against a node from
    /// the last [`accessibility_snapshot`](Self::accessibility_snapshot) — the
    /// operable counterpart to that (perceivable) walk.
    ///
    /// `node_id` is the id the AT names, resolved through
    /// [`crate::accessibility::access_target`]: a widget, one option inside a
    /// composite widget, or the window root (never actionable). Returns whether
    /// anything acted on it.
    ///
    /// Three rules keep an AT inside what a mouse or keyboard could already do:
    ///
    /// - **Stale / invisible targets are refused.** The tree can rebuild
    ///   between the snapshot an AT read and the action it sends back, so an id
    ///   may name a tombstoned slot; that is a miss, not a panic.
    /// - **A modal layer confines actions**, exactly as it confines pointer and
    ///   key dispatch: while one is up, a target outside its subtree is inert.
    ///   The snapshot already flags the modal for the AT — this enforces it.
    /// - **Activating a focusable control focuses it first**, mirroring the
    ///   click path, so the ring and any focus-driven widget state end up where
    ///   they would after a mouse press.
    ///
    /// [`Focus`](AccessAction::Focus) is handled here rather than by the widget
    /// — the tree owns the `FocusManager`. Everything else routes to
    /// [`Widget::accessibility_action`]. Tree mutations queued by whatever
    /// handler runs are drained before returning, exactly as in
    /// [`dispatch_event`](Self::dispatch_event).
    pub fn perform_access_action(
        &mut self,
        node_id: u64,
        action: AccessAction,
        event_ctx: &mut EventContext,
    ) -> bool {
        let (idx, option) = match access_target(node_id) {
            AccessTarget::Window => return false,
            AccessTarget::Widget(idx) => (idx, None),
            AccessTarget::Option { owner, index } => (owner, Some(index)),
        };

        // The id came from a snapshot that may be a frame or more old.
        if !self.node(idx).is_some_and(|n| n.widget.visible()) {
            return false;
        }
        if !self.reachable_for_access(idx) {
            return false;
        }

        if action == AccessAction::Focus {
            if !self.widget(idx).focusable() {
                return false;
            }
            self.focus(Some(idx), event_ctx);
            self.drain_commands(event_ctx);
            return true;
        }

        if self.widget(idx).focusable() && self.focus.focused() != Some(idx) {
            self.focus(Some(idx), event_ctx);
        }

        let layout = self.layout_rect(idx);
        let result = self
            .widget_mut(idx)
            .accessibility_action(action, option, layout, event_ctx);
        self.drain_commands(event_ctx);
        result == EventResult::Consumed
    }

    /// Whether `idx` sits in the subtree that currently owns input — the
    /// topmost interactive layer if one is up, otherwise anywhere. The same
    /// confinement `dispatch_event` applies to pointer and key events, so an
    /// AT cannot operate a widget behind a modal that the user can't click.
    fn reachable_for_access(&self, idx: usize) -> bool {
        let Some(layer) = self.topmost_interactive_layer() else {
            return true;
        };
        let layer_root = layer.root;
        self.ancestors_inclusive(Some(idx)).contains(&layer_root)
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
        // Remember where the cursor is, in viewport space (the shift into the
        // target subtree's frame happens further down). `resync_hover` replays
        // this position when a layer pop changes what sits under a cursor that
        // is not moving, since nothing else recomputes the hover chain.
        if let Some(pos) = event_position(event) {
            self.last_pointer_pos = Some(pos);
        }

        // Pick the subtree that owns this event: the topmost *interactive*
        // layer if any (a click-through tooltip overlay on top is skipped),
        // otherwise the main root. `offset` translates viewport coords into
        // the subtree's local space (zero for the main tree).
        let (target_root, offset, layer_active) = match self.topmost_interactive_layer() {
            Some(layer) => (Some(layer.root), layer.offset, true),
            None => (self.root, (0.0, 0.0), false),
        };

        // Publish the active layer's offset so handlers that push an
        // `AnchorRect` popover from inside this layer (e.g. a `Dropdown` in a
        // modal) get their layer-local trigger rect translated to viewport
        // coordinates — see `EventContext::push_layer`. Drain runs while this
        // is still set (a re-entrant Focus handler stays in the same space).
        event_ctx.current_layer_offset = offset;

        let result = self.dispatch_with_target(target_root, offset, layer_active, event, event_ctx);

        // Drain tree-mutation commands queued by handlers. Loops because
        // `TreeCommand::Focus` re-enters `Widget::event` (FocusLost/Gained)
        // and those handlers can themselves enqueue more commands —
        // `drain_commands` keeps going until the queue settles.
        self.drain_commands(event_ctx);

        // `EventContext` is reused across dispatches and other entry points
        // (on_frame, file drop, focus flush) that run with no active layer;
        // reset so those see viewport coordinates rather than this layer's.
        event_ctx.current_layer_offset = (0.0, 0.0);

        result
    }

    /// Register a handler for OS file drops onto the window (drag-and-drop
    /// from the desktop / file manager).
    ///
    /// Fires once per file dropped — winit delivers a multi-file drop as a
    /// burst of separate events. The handler receives the file's path and
    /// the event context; enqueue tree commands on the context (e.g.
    /// `ctx.rebuild_children(...)`) to mutate the tree in response, exactly
    /// like a widget event handler.
    ///
    /// The registration is **screen-scoped**: it is cleared automatically
    /// on the next [`EventContext::replace_screen`] transition, so a
    /// handler installed by the current screen never fires after that
    /// screen is replaced. Re-register inside each screen's build closure
    /// that wants to accept drops. Only one handler is supported — a second
    /// call replaces the first.
    ///
    /// **No drop position is provided.** winit 0.30 carries no coordinates
    /// on `DroppedFile` and stops emitting cursor-move events during an OS
    /// drag, so a reliable per-widget hit-test isn't possible (notably on
    /// Windows). Treat a drop as "a file arrived on the window" and route
    /// it from app state — e.g. insert it into the currently open document.
    pub fn on_file_drop(&mut self, handler: impl FnMut(&Path, &mut EventContext) + 'static) {
        self.file_drop_handler = Some(Box::new(handler));
    }

    /// Deliver an OS file drop to the registered [`Self::on_file_drop`]
    /// handler, if any, then drain whatever tree commands it enqueued.
    ///
    /// Called by the event loop on `WindowEvent::DroppedFile`. A no-op when
    /// no handler is registered (e.g. on a screen that doesn't accept
    /// drops). The handler borrows `&mut self.file_drop_handler` for the
    /// call while `event_ctx` (an independent borrow) carries the deferred
    /// command queue — handlers can't re-enter `WidgetTree` directly, so
    /// there's no take/restore dance.
    pub fn dispatch_file_drop(&mut self, path: &Path, event_ctx: &mut EventContext) {
        if let Some(handler) = self.file_drop_handler.as_mut() {
            handler(path, event_ctx);
        }
        self.drain_commands(event_ctx);
    }

    /// Register a handler for an image pasted from the system clipboard.
    ///
    /// Fired when the user presses Ctrl/Cmd+V while the clipboard holds an
    /// image rather than text (the event loop tries text first). The handler
    /// receives the image as encoded PNG bytes — the same self-describing
    /// shape the file-drop path hands over — so an app can store or insert it
    /// without knowing the clipboard's raw pixel format.
    ///
    /// Like [`Self::on_file_drop`] this is a window-level hook (clipboard
    /// content carries no drop position) and is screen-scoped: it is cleared
    /// on `replace_screen`, so register it in each screen's build closure.
    pub fn on_image_paste(&mut self, handler: impl FnMut(&[u8], &mut EventContext) + 'static) {
        self.image_paste_handler = Some(Box::new(handler));
    }

    /// Deliver a clipboard image paste to the registered
    /// [`Self::on_image_paste`] handler, if any, then drain whatever tree
    /// commands it enqueued.
    ///
    /// Called by the event loop when a paste combo finds image bytes on the
    /// clipboard. A no-op when no handler is registered (e.g. a screen that
    /// doesn't accept pasted images). Mirrors [`Self::dispatch_file_drop`]'s
    /// borrow split: the handler borrows `&mut self.image_paste_handler`
    /// while `event_ctx` carries the deferred command queue.
    pub fn dispatch_image_paste(&mut self, png: &[u8], event_ctx: &mut EventContext) {
        if let Some(handler) = self.image_paste_handler.as_mut() {
            handler(png, event_ctx);
        }
        self.drain_commands(event_ctx);
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
            let top = self
                .topmost_interactive_layer()
                .expect("layer_active implies an interactive layer");
            let dismiss = top.options.dismiss_on_escape;
            let root = top.root;
            if dismiss {
                // Pop the active interactive layer specifically — a
                // click-through tooltip may be stacked above it, and
                // `pop_top_layer` would yank that instead.
                self.pop_layer(root);
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

        // ↓ / ↑ step *into* a layer that owns the keyboard but has nothing
        // in it focused. That is the state every menu opens in: opening
        // deliberately leaves focus out on the trigger (pre-focusing a row
        // would highlight it, which native menus don't), and keys go to the
        // layer's subtree — so the arrows reach no listener at all and the
        // one thing they can sensibly mean here is what Tab means, enter the
        // layer. `Backward` enters at the last stop, matching Shift+Tab and a
        // native menu opened with ↑.
        //
        // Gated on focus being *outside* the layer, which is what keeps this
        // from shadowing any widget that wants the arrows for itself: a
        // focused `MenuItem` steps rows with them, an `Input` in a modal moves
        // its caret, and both keep them, because the moment focus is inside
        // the guard is false and the event routes normally.
        //
        // Only the arrows, deliberately — not Enter. Entering is navigation
        // and shows the user where they landed; activating a row nobody has
        // seen highlighted is a different thing entirely (see
        // `menu_keyboard_tests::an_unfocused_menu_swallows_enter_...`).
        if layer_active {
            let dir = match event {
                WidgetEvent::KeyDown {
                    key: Key::Named(NamedKey::ArrowDown),
                } => Some(FocusDirection::Forward),
                WidgetEvent::KeyDown {
                    key: Key::Named(NamedKey::ArrowUp),
                } => Some(FocusDirection::Backward),
                _ => None,
            };
            if let Some(dir) = dir {
                let layer_root = self
                    .topmost_interactive_layer()
                    .expect("layer_active implies an interactive layer")
                    .root;
                let focus_inside = self
                    .focus
                    .focused()
                    .map(|f| self.ancestors_inclusive(Some(f)).contains(&layer_root))
                    .unwrap_or(false);
                if !focus_inside {
                    self.advance_focus(dir, event_ctx);
                    return EventResult::Consumed;
                }
            }
        }

        // Pointer capture: a widget that began a drag (e.g. an `Input`
        // mid-selection) receives every `MouseMove` / `MouseUp` directly,
        // ahead of hit-testing and the layer routing below, so the drag keeps
        // extending — and is reliably ended — even when the cursor leaves the
        // widget's rect or the active layer. `MouseDown` is never captured: a
        // fresh press routes normally (and is where capture is acquired).
        if let Some(cap) = self.pointer_capture {
            if !self.contains(cap) {
                self.pointer_capture = None;
            } else if matches!(
                event,
                WidgetEvent::MouseMove { .. } | WidgetEvent::MouseUp { .. }
            ) {
                let (dx, dy) = self.pointer_offset_for(cap, offset);
                let local = shift_event_position(event, dx, dy);
                let layout_rect = self.layout_rect(cap);
                let result = self
                    .node_mut(cap)
                    .expect("capture liveness checked above")
                    .widget
                    .event(&local, layout_rect, event_ctx);
                self.apply_capture_change(cap, event_ctx);
                event_ctx.stamp_pending_layer_opener(cap);
                return result;
            }
        }

        // Pointer events: when a layer is up, route based on whether the
        // cursor is inside the layer's interactive rect. Outside hits
        // either dismiss (configurable) or are silently swallowed, but
        // never fall through to the main tree.
        if layer_active && let Some(pos) = event_position(event) {
            // `pos` is viewport-space here — events are shifted by
            // `-offset` into the target subtree's frame only *after* this
            // block, so every layer's rect is tested in the same space
            // (`entry.offset` + `entry.measured_size`, both from the last
            // layout pass).
            let point = Point::new(pos.x, pos.y);
            // The active interactive layer (not necessarily the literal
            // topmost — a tooltip may paint above it). `offset` above
            // came from the same entry, so its rect agrees.
            let top = self
                .topmost_interactive_layer()
                .expect("layer_active implies an interactive layer");
            let top_rect = Rect::new(
                top.offset.0,
                top.offset.1,
                top.measured_size.width,
                top.measured_size.height,
            );
            // A scrim means "modal": everything behind it is blocked, so
            // menu-switch pass-through (hover + one-click switch onto a
            // trigger behind the layer) is disabled. Only chrome-less
            // popovers (no scrim — dropdowns / toolbar menus) let a peer
            // trigger stay live underneath.
            let top_has_scrim = top.options.scrim.is_some();
            if !top_rect.contains(point) {
                // Outside the topmost interactive layer's content rect.
                if let WidgetEvent::MouseMove { .. } = event {
                    // Cursor over scrim / margin: normally drop any
                    // layer-side hover (so widgets get a final MouseLeave)
                    // without dismissing. Exception: a menu-switch trigger
                    // sitting in the main tree (a peer toolbar button) stays
                    // live, so it lights up on hover — without this a
                    // one-click-switchable button gives no "pressable"
                    // affordance while a sibling menu is open. Route the
                    // move to the main tree so that trigger (and only it)
                    // gets MouseEnter; moving back into the menu leaves it
                    // again via the normal layer routing below (hover
                    // enter/leave is LCA-based, so it spans the boundary).
                    if !top_has_scrim && let Some(main) = self.root {
                        let over_switch = self
                            .hit_test_in(main, point)
                            .and_then(|h| self.node(h))
                            .map(|n| n.widget.menu_switch_trigger())
                            .unwrap_or(false);
                        if over_switch {
                            self.update_hover_in(main, point, event_ctx);
                            return EventResult::Ignored;
                        }
                    }
                    self.clear_hover(event_ctx);
                    return EventResult::Ignored;
                }
                if let WidgetEvent::MouseDown { .. } = event {
                    // Location-aware collapse: a pointer carries a
                    // position, and that position encodes how deep to
                    // dismiss. Walk the interactive layers top→down and
                    // pop the run that (a) does not contain the click and
                    // (b) opts into outside-click dismiss, stopping at the
                    // first layer that contains the click or refuses to
                    // dismiss — that layer, and everything below it, stays.
                    //
                    // So a click in dead space outside the whole stack
                    // collapses all of it in one go, while a click that
                    // lands inside an *intermediate* layer (e.g. the
                    // popover body beneath an open dropdown) peels only the
                    // layers stacked above it. Escape, by contrast, always
                    // peels a single level (a keystroke has no location) —
                    // see the Escape interceptor above.
                    //
                    // Testing each layer's *own* rect is what makes a
                    // dropdown that overflows its parent popover behave:
                    // the click is judged against the dropdown's rect and
                    // the popover's rect independently, not against a
                    // parent box that visually swallows the child.
                    let mut to_pop: Vec<usize> = Vec::new();
                    // Openers of the layers being dismissed, so the
                    // menu-switch path below can tell "clicked the trigger
                    // that owns a just-closed menu" (toggle closed — don't
                    // reopen) from "clicked a peer trigger" (switch).
                    let mut popped_openers: Vec<usize> = Vec::new();
                    for entry in self.layers.iter().rev() {
                        if !entry.options.interactive {
                            // Click-through overlay (tooltip): never a
                            // dismiss target, and it doesn't shield the
                            // interactive layers beneath it.
                            continue;
                        }
                        let rect = Rect::new(
                            entry.offset.0,
                            entry.offset.1,
                            entry.measured_size.width,
                            entry.measured_size.height,
                        );
                        if rect.contains(point) || !entry.options.dismiss_on_outside_click {
                            // The click landed inside this layer, or this
                            // layer eats outside clicks without closing
                            // (a non-dismissable modal). Stop: it and
                            // everything below stay.
                            break;
                        }
                        to_pop.push(entry.root);
                        if let Some(opener) = entry.opener {
                            popped_openers.push(opener);
                        }
                    }
                    for root in to_pop {
                        self.pop_layer(root);
                    }

                    // Menu-switch: if the dismissing click landed on a
                    // widget that opts into the one-click switch (a peer
                    // menu's trigger — e.g. a toolbar gear/overflow),
                    // re-route this very MouseDown to the now-exposed tree
                    // instead of swallowing it, so the trigger presses and
                    // opens its own menu in the same click. Any other
                    // target (a consequential button, dead space) still
                    // swallows, so this is not a general pointer
                    // pass-through — only opted-in menu triggers get it.
                    //
                    // The exposed target is recomputed exactly as
                    // `dispatch_event` does; `menu_switch_trigger` is
                    // hit-tested in that subtree's local space. Re-entry is
                    // bounded: the trigger sits inside the exposed target's
                    // rect (that is how the hit-test found it), so the
                    // recursive dispatch skips this outside-click branch and
                    // routes the press normally. A click on the trigger that
                    // *owns* a just-closed menu is excluded (it stays a plain
                    // toggle-closed) — re-routing there would immediately
                    // reopen the menu it just dismissed.
                    let (sw_target, sw_offset, sw_active) = match self.topmost_interactive_layer() {
                        Some(layer) => (Some(layer.root), layer.offset, true),
                        None => (self.root, (0.0, 0.0), false),
                    };
                    let is_primary = matches!(
                        event,
                        WidgetEvent::MouseDown {
                            button: MouseButton::Left,
                            ..
                        }
                    );
                    if is_primary
                        && !top_has_scrim
                        && let Some(root) = sw_target
                    {
                        let local = Point::new(point.x - sw_offset.0, point.y - sw_offset.1);
                        let hit = self.hit_test_in(root, local);
                        let is_switch = hit
                            .and_then(|h| self.node(h))
                            .map(|n| n.widget.menu_switch_trigger())
                            .unwrap_or(false);
                        let owns_closed_menu =
                            hit.map(|h| popped_openers.contains(&h)).unwrap_or(false);
                        if is_switch && !owns_closed_menu {
                            event_ctx.current_layer_offset = sw_offset;
                            // Press the trigger with the real down...
                            self.dispatch_with_target(
                                sw_target, sw_offset, sw_active, event, event_ctx,
                            );
                            // ...then fire it now with a synthetic
                            // release, so the new menu opens on this same
                            // physical press. Closing the old menu (above)
                            // and opening the new one land in one
                            // dispatch, so no frame shows an empty gap —
                            // the "flicker" a real down→up wait exposes.
                            // The trailing real MouseUp arrives with the
                            // new menu already up and fires nothing,
                            // wherever it lands: outside the menu it is
                            // swallowed, and inside it every activatable
                            // widget needs a press first — `release`
                            // without one is `Release::Idle` (see
                            // [`InteractionState`]). So this does not rely
                            // on where a menu anchors relative to its
                            // trigger.
                            let release = WidgetEvent::MouseUp {
                                position: point,
                                button: MouseButton::Left,
                            };
                            return self.dispatch_with_target(
                                sw_target, sw_offset, sw_active, &release, event_ctx,
                            );
                        }
                    }
                }
                // MouseUp / Scroll / other position-bearing events
                // outside the layer are silently swallowed.
                return EventResult::Consumed;
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
            // Pointer-driven focus: suppress the ring (:focus-visible).
            self.focus_with_reason(new_focus, FocusReason::Pointer, event_ctx);
            if new_focus.is_none() {
                // A click on something that cannot take focus still says where
                // the user is. Focus clears, but the *place* survives as the
                // start point for the next Tab — otherwise clicking a note's
                // background sends Tab back to the top of the app, and the user
                // learns not to trust it. `hit` is `None` only for a click that
                // lands outside the tree entirely, which names no place, so
                // assigning it through is right: no anchor, ends of the order.
                self.focus.set_nav_start(hit);
            }
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

    /// Whether the focused widget should paint a focus ring — the
    /// `:focus-visible` state the paint pass publishes each frame.
    ///
    /// `false` when nothing is focused, and when focus was acquired by
    /// pointing at the widget. The companion to [`Self::focused`]: focus is
    /// two facts, where it is and whether it shows, and code that restores
    /// focus across a rebuild has to reason about both.
    pub fn focus_visible(&self) -> bool {
        self.focus.visible()
    }

    /// Index of the widget that currently holds the pointer capture, if any.
    ///
    /// Set when a handler calls [`EventContext::capture_pointer`] (e.g. an
    /// `Input` starting a drag-select) and cleared on
    /// [`EventContext::release_pointer`] or when the widget is removed. While
    /// `Some`, `MouseMove` / `MouseUp` are delivered straight to this widget.
    pub fn pointer_capture(&self) -> Option<usize> {
        self.pointer_capture
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
        self.tab_order_with_cut(None).0
    }

    /// Tab order, plus where `nav_start` falls within it.
    ///
    /// The cut is a *caret between* tab stops, not a stop itself: it counts
    /// the focusables that precede `nav_start` in tree order, so a cut of `k`
    /// means Tab goes to `order[k]` and Shift+Tab to `order[k - 1]`. That is
    /// the shape the web platform gives its sequential focus navigation
    /// starting point, and it is the reason a non-focusable widget can name a
    /// position in an order it does not appear in.
    ///
    /// `None` when `nav_start` is `None`, or when the node it names is gone,
    /// hidden, or outside the active subtree (e.g. behind a layer opened
    /// since the click) — callers then fall back to the ends of the order.
    ///
    /// One walk produces both so the two can never disagree about which
    /// subtree to start from or which branches are visible.
    fn tab_order_with_cut(&self, nav_start: Option<usize>) -> (Vec<usize>, Option<usize>) {
        let mut out = Vec::new();
        let mut cut = None;
        // Trap Tab inside the active interactive layer, skipping any
        // click-through tooltip painted on top (it owns no focus).
        let start = self
            .topmost_interactive_layer()
            .map(|l| l.root)
            .or(self.root);
        if let Some(root) = start {
            self.collect_focusable(root, nav_start, &mut out, &mut cut);
        }
        (out, cut)
    }

    fn collect_focusable(
        &self,
        idx: usize,
        nav_start: Option<usize>,
        out: &mut Vec<usize>,
        cut: &mut Option<usize>,
    ) {
        let Some(node) = self.node(idx) else { return };
        if !node.widget.visible() {
            return;
        }
        // Before the push below: the cut sits *ahead of* this node, so a
        // (hypothetical) focusable start point would be the next stop rather
        // than one that has already gone by.
        if nav_start == Some(idx) {
            *cut = Some(out.len());
        }
        if node.widget.focusable() {
            out.push(idx);
        }
        // Clone to avoid holding an immut borrow across the recursion;
        // child lists are short enough that this is cheap.
        let children: Vec<usize> = node.children.clone();
        for c in children {
            self.collect_focusable(c, nav_start, out, cut);
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
    /// With nothing focused the step starts from the last click that could
    /// not take focus — clicking a note's background and pressing Tab
    /// continues into that note, rather than restarting at the top of the
    /// window. Failing that (no click yet, or one whose target has since
    /// been removed, hidden, or shut behind a layer) it starts from the end
    /// `dir` comes from: first stop for `Forward`, last for `Backward`.
    ///
    /// Invoked by the tree's own Tab routing — callers rarely need
    /// this directly, but it is public so tests and custom shortcut
    /// bindings can reuse the traversal policy.
    pub fn advance_focus(
        &mut self,
        dir: FocusDirection,
        event_ctx: &mut EventContext,
    ) -> Option<usize> {
        let (order, cut) = self.tab_order_with_cut(self.focus.nav_start());
        if order.is_empty() {
            return None;
        }

        let next = match (self.focus.focused(), dir) {
            // Nothing focused, but the user clicked something along the way:
            // resume from where they pointed instead of the top of the order.
            // `cut` is a position *between* stops, so the two directions read
            // it symmetrically, and the modulo gives the same wrap as a step
            // off either end.
            (None, dir) => match cut {
                Some(c) => {
                    let n = order.len();
                    match dir {
                        FocusDirection::Forward => order[c % n],
                        FocusDirection::Backward => order[(c + n - 1) % n],
                    }
                }
                None => match dir {
                    FocusDirection::Forward => order[0],
                    FocusDirection::Backward => *order.last().unwrap(),
                },
            },
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

        // Keyboard-driven focus: the ring is shown so a Tab user can see
        // where focus went.
        self.focus_with_reason(Some(next), FocusReason::Keyboard, event_ctx);
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
        // App-facing focus is programmatic: the user didn't point at the
        // widget, so the ring shows (like keyboard navigation).
        self.focus_with_reason(new, FocusReason::Programmatic, event_ctx)
    }

    /// Focus `new`, recording *why* focus moved so the `:focus-visible`
    /// heuristic can decide whether to paint a ring. The built-in
    /// click-to-focus path passes [`FocusReason::Pointer`] (ring
    /// suppressed) and Tab routing passes [`FocusReason::Keyboard`]; the
    /// public [`focus`](Self::focus) wrapper passes
    /// [`FocusReason::Programmatic`]. Otherwise identical to `focus`,
    /// including the `FocusLost`/`FocusGained` dispatch.
    fn focus_with_reason(
        &mut self,
        new: Option<usize>,
        reason: FocusReason,
        event_ctx: &mut EventContext,
    ) -> Option<usize> {
        let prev = self.focus.set(new);
        // Refresh visibility even on a same-widget re-focus (set early so a
        // click on an already-keyboard-focused widget drops its ring) —
        // this is before the no-op early return below.
        self.focus.set_visible(new.is_some() && reason.shows_ring());
        // Arm the scroll-into-view for the same reason, and for the same
        // "even if focus didn't move" logic: Tab with a single stop re-focuses
        // the widget it is already on, and the user still expects to be shown
        // where that is.
        self.pending_reveal = new.filter(|_| reason.scrolls_into_view());
        if prev == new {
            return prev;
        }

        if let Some(p) = prev
            && let Some(n) = self.node(p)
        {
            let rect = self.layout.absolute_rect(n.layout_node);
            if let Some(node) = self.node_mut(p) {
                node.widget.event(&WidgetEvent::FocusLost, rect, event_ctx);
            }
        }

        if let Some(n) = new
            && let Some(nd) = self.node(n)
        {
            let rect = self.layout.absolute_rect(nd.layout_node);
            if let Some(node) = self.node_mut(n) {
                node.widget
                    .event(&WidgetEvent::FocusGained, rect, event_ctx);
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

    /// Re-resolve the hover chain against the last known cursor position,
    /// emitting the `MouseLeave` / `MouseEnter` a real `MouseMove` would have.
    /// Returns `true` when the hover chain actually changed.
    ///
    /// A no-op unless a layer popped, the hovered widget was removed, or a
    /// scroll view glided content under the cursor since the last call.
    /// `hovered` is normally only ever updated from a live `MouseMove`, which
    /// leaves a hole: dismissing a menu re-exposes whatever sits under the
    /// cursor, but if the cursor does not then move, nothing tells that widget
    /// it is hovered. It is most visible when the dismiss *is* the click on
    /// another button — the outside-click path swallows that press, so the
    /// button under the pointer receives neither the click nor a hover until
    /// the mouse jiggles.
    ///
    /// Removal is the same hole reached from the other side: a button whose
    /// handler rebuilds the list it lives in activates itself out of
    /// existence, and the replacement lands under the unmoved cursor with no
    /// `MouseMove` to light it. A wheel scroll is the same hole again: the
    /// content, not the tree, is what moved, sliding a different row under the
    /// still pointer (see `sync_scroll_view_content_heights`, which arms the
    /// resync each gliding frame). Hover is purely geometric, so unlike the
    /// focus counterpart this needs nothing from the builder — the replay
    /// re-derives it from the cursor position alone.
    ///
    /// The mirror case on push is deliberately not handled here: opening a
    /// layer clears hover (see the `PushLayer` arm of `apply_commands`) and
    /// leaves it cleared, so a context menu that pops up under the cursor does
    /// not pre-highlight the item it happens to land on — matching native menus
    /// on Windows and macOS.
    ///
    /// Call once per frame *after* layout, so the hit-test sees fresh
    /// geometry: a handler that popped the layer may also have rebuilt the
    /// tree beneath it (choosing a dropdown item that reorders the list), and
    /// those nodes have no resolved rect until the layout pass runs. Deferred
    /// while the pointer is captured — a drag owns the pointer and must not be
    /// interrupted by a hover change — and skipped before the first pointer
    /// event of the session, when the cursor's position is simply unknown.
    pub fn resync_hover(&mut self, event_ctx: &mut EventContext) -> bool {
        // Test the capture *before* consuming `hover_dirty`: a drag defers the
        // resync, it does not cancel it. Taking the flag first would burn it on
        // a frame that declines to use it, so a rebuild landing mid-drag would
        // leave the replacement dark even once the drag released — the exact
        // hole this resync exists to close.
        if self.pointer_capture.is_some() {
            return false;
        }
        if !std::mem::take(&mut self.hover_dirty) {
            return false;
        }
        // Unlike the capture, this needs no deferral: `last_pointer_pos` is
        // only `None` before the first pointer event, and the event that
        // clears it is a `MouseMove` that resolves hover on its own.
        let Some(pos) = self.last_pointer_pos else {
            return false;
        };

        let before = self.hovered;
        // Same routing rule as `dispatch_event`: an interactive layer still
        // standing owns the pointer, so hover is resolved inside it (in its
        // local frame) and the cursor counts as hovering nothing when it sits
        // outside that layer's rect. Copy the entry's fields out to end the
        // borrow before the `&mut self` calls below.
        let layer = self
            .topmost_interactive_layer()
            .map(|l| (l.root, l.offset, l.measured_size));
        match layer {
            Some((root, offset, size)) => {
                let rect = Rect::new(offset.0, offset.1, size.width, size.height);
                if rect.contains(pos) {
                    let local = Point::new(pos.x - offset.0, pos.y - offset.1);
                    self.update_hover_in(root, local, event_ctx);
                } else {
                    self.clear_hover(event_ctx);
                }
            }
            None => match self.root {
                Some(root) => self.update_hover_in(root, pos, event_ctx),
                None => self.clear_hover(event_ctx),
            },
        }

        // `MouseEnter` handlers enqueue commands like any other handler
        // (a hoverable row that opens a tooltip layer, say).
        self.drain_commands(event_ctx);
        self.hovered != before
    }

    /// Show the pending hover tooltip once its trigger has been rested on for
    /// the configured delay. Returns `true` when a bubble was pushed, so the
    /// caller can lay out again before painting — the fresh layer has no
    /// geometry until it does.
    ///
    /// Call once per frame, after layout. Cheap on an idle frame: one
    /// thread-local borrow that finds nothing armed. While a tip is armed but
    /// not yet due this votes for a frame at the deadline, so the delay does
    /// not depend on the app running its own periodic tick.
    ///
    /// Dismissal is not here — that rides on the trigger's `MouseLeave`, and
    /// on [`Self::remove`] for the teardown paths that produce none.
    pub fn sync_tooltips(&mut self) -> bool {
        let Some(pending) = crate::tooltip::due_now() else {
            return false;
        };
        let (options, bubble) = crate::tooltip::bubble_for(&pending);
        let root = self.push_layer(options, bubble);
        crate::tooltip::mark_shown(root);
        true
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
        if let Some(pos) = event_position(event)
            && !layout_rect.contains(pos)
        {
            return EventResult::Ignored;
        }

        // Borrow widget mutably (safe because we're not accessing children here)
        let result = {
            let Some(node) = self.node_mut(idx) else {
                return EventResult::Ignored;
            };
            node.widget.event(event, layout_rect, event_ctx)
        };
        // Bind any pointer-capture request the handler made to this node (so a
        // `MouseDown` that starts a drag captures the pointer for it), and
        // stamp this node as the opener of any layer it just pushed (so the
        // menu-switch path can distinguish the owning trigger from a peer).
        self.apply_capture_change(idx, event_ctx);
        event_ctx.stamp_pending_layer_opener(idx);
        result
    }

    /// Apply a pointer-capture request a widget made during its `event`
    /// handler. `node` is the widget that just handled the event, so an
    /// acquire binds the capture to it; a release clears the capture only
    /// when it currently belongs to that node.
    fn apply_capture_change(&mut self, node: usize, event_ctx: &mut EventContext) {
        match event_ctx.take_capture_change() {
            Some(true) => self.pointer_capture = Some(node),
            // Only the capturing widget can release its own capture.
            Some(false) if self.pointer_capture == Some(node) => self.pointer_capture = None,
            _ => {}
        }
    }

    /// Coordinate delta that maps a viewport-space pointer position into
    /// `idx`'s local event space — the same shift the recursive dispatch
    /// accumulates on the way down: minus the active layer `offset`, plus
    /// every proper ancestor's scroll offset. Used to deliver captured
    /// pointer events (which skip the walk) in the widget's own space.
    fn pointer_offset_for(&self, idx: usize, layer_offset: (f32, f32)) -> (f32, f32) {
        let mut dx = -layer_offset.0;
        let mut dy = -layer_offset.1;
        let mut cur = self.node(idx).and_then(|n| n.parent);
        while let Some(a) = cur {
            let Some(node) = self.node(a) else { break };
            let (sx, sy) = node.widget.scroll_offset();
            dx += sx;
            dy += sy;
            cur = node.parent;
        }
        (dx, dy)
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

    /// Consume the "root was replaced" flag, returning whether a root swap has
    /// happened since the last call and clearing it.
    ///
    /// The event loop calls this once before each layout to decide whether to
    /// drop the text engine's shape cache (a screen swap, e.g. a vault lock,
    /// invalidates the previous screen's cached glyph geometry).
    pub fn take_root_replaced(&mut self) -> bool {
        std::mem::take(&mut self.root_replaced)
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
    query: sindon_layout::MeasureQuery,
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
