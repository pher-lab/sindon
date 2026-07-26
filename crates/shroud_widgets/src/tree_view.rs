//! Tree view — a collapsible, indented list of hierarchical rows.
//!
//! A [`TreeView`] renders a [`TreeItem`] forest as a flat vertical list of rows,
//! each indented by its depth, with a disclosure chevron on any row that has
//! children. Clicking a chevron expands or collapses that node; clicking a row
//! selects it. Selection and expansion are internal state, observable through
//! [`TreeView::on_select`].
//!
//! # Shape
//!
//! The visible rows are driven by a [`ReactiveChildren`](crate::ReactiveChildren):
//! expanding or collapsing bumps a revision token, and the row list is rebuilt to
//! match the newly visible set — the framework's standard reactive-rebuild path,
//! so the tree needs no manual child bookkeeping. Each row is a self-painting
//! [`Widget`] that reads the active theme, so selection and hover track the theme
//! with no per-row color wiring.
//!
//! # Keyboard
//!
//! The whole tree is a **single tab stop** (the ARIA tree pattern): focus lands on
//! an invisible host that owns the rows, and the arrow keys move a *roving cursor*
//! between them. ↑/↓ walk the visible rows (clamped, no wrap), Home/End jump to
//! the ends, → opens a closed node and then descends into it, ← collapses an open
//! one and otherwise climbs to the parent, and Enter / Space commit the cursor row
//! as the selection. Typing letters jumps to the next row whose label starts with
//! what you typed.
//!
//! The cursor is deliberately *not* the selection: arrowing moves it without
//! firing [`on_select`](TreeView::on_select), so walking a tree whose handler
//! loads something expensive stays cheap. Commit with Enter / Space.
//!
//! Keeping focus on the host (rather than making every row a tab stop) is also
//! what makes expansion safe: the row list is rebuilt whenever a node opens or
//! closes, and a focused *row* would be tombstoned by its own toggle. The host is
//! the rows' parent, so it survives every rebuild, and the cursor rides on shared
//! state rather than on any one row widget.
//!
//! # Building
//!
//! ```ignore
//! let items = vec![
//!     TreeItem::new(1, "src").child(TreeItem::new(2, "main.rs")),
//!     TreeItem::new(3, "README.md"),
//! ];
//! TreeView::new(items)
//!     .expanded([1])                       // start with `src` open
//!     .on_select(|id, _ctx| println!("selected {id}"))
//!     .build(&mut tree, parent);
//! ```
//!
//! # Scope
//!
//! Mouse and keyboard are both wired; OS a11y tree exposure is the remaining
//! deliberate follow-up — the [`AccessRole`](shroud_core::AccessRole) vocabulary
//! has no tree role yet, so a screen reader still sees the rows as plain groups.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::event::{EventContext, EventResult, Key, MouseButton, NamedKey, WidgetEvent};
use crate::interaction::{InteractionState, step_selection};
use crate::paint::PaintContext;
use crate::reactive_children::ReactiveChildren;
use crate::tree::WidgetTree;
use crate::widget::Widget;
use shroud_core::{Color, Rect};
use shroud_layout::FlexStyle;
use shroud_text::TextAttrs;

const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 16.0;
const LEFT_PAD: f32 = 8.0;
const CHEVRON_W: f32 = 16.0;
const LABEL_GAP: f32 = 4.0;
const ROW_RADIUS: f32 = 5.0;
/// How long a type-ahead search buffer survives between keystrokes. Past this,
/// the next letter starts a fresh search instead of extending the old one — the
/// usual list-box convention, so a letter typed a minute later means "jump to
/// L", not "keep looking for `foobarl`".
const TYPE_AHEAD_TIMEOUT: Duration = Duration::from_millis(1000);

/// A node in a [`TreeView`] model: a stable `id`, a `label`, and any children.
///
/// Build a leaf with [`TreeItem::new`] and attach children with
/// [`child`](TreeItem::child) (or pass them all at once to
/// [`with_children`](TreeItem::with_children)). Ids must be unique across the
/// whole forest — they key selection and expansion.
#[derive(Debug, Clone)]
pub struct TreeItem {
    id: u64,
    label: String,
    children: Vec<TreeItem>,
}

impl TreeItem {
    /// A leaf (no children) with a unique `id` and display `label`.
    pub fn new(id: u64, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            children: Vec::new(),
        }
    }

    /// A node with `children` given up front.
    pub fn with_children(id: u64, label: impl Into<String>, children: Vec<TreeItem>) -> Self {
        Self {
            id,
            label: label.into(),
            children,
        }
    }

    /// Append a child (builder style).
    pub fn child(mut self, child: TreeItem) -> Self {
        self.children.push(child);
        self
    }

    fn has_children(&self) -> bool {
        !self.children.is_empty()
    }
}

/// Callback fired when a row is selected: the row's id and the dispatch context.
type SelectHandler = Box<dyn FnMut(u64, &mut EventContext)>;

/// One currently-visible row, flattened out of the model: everything the row
/// builder and the keyboard navigation both need, so neither has to re-walk the
/// forest on its own terms.
#[derive(Debug, Clone, PartialEq)]
struct RowInfo {
    id: u64,
    label: String,
    depth: usize,
    has_children: bool,
    expanded: bool,
    /// The row's parent id, `None` at the top level. What ← climbs to.
    parent: Option<u64>,
}

/// State shared by the host, the row builder and every row widget: the model,
/// the expansion / selection sets, the roving cursor, a revision the
/// [`ReactiveChildren`] watches, and the selection callback.
struct TreeState {
    items: Vec<TreeItem>,
    expanded: RefCell<HashSet<u64>>,
    selected: Cell<Option<u64>>,
    /// The roving cursor — the row the arrow keys move from, which is *not* the
    /// selection (see the module docs). Lives here rather than on a row widget
    /// so it survives the rebuild an expand/collapse triggers.
    active: Cell<Option<u64>>,
    /// Whether the host currently holds keyboard focus. Gates the cursor ring
    /// and every key binding.
    focused: Cell<bool>,
    /// Tree index of the focusable host, stamped at build time. Lets a row hand
    /// the keyboard back to the host when clicked.
    host: Cell<Option<usize>>,
    /// Row id → tree index, rewritten by every rebuild. The reveal needs a node
    /// index and the cursor only knows an id; keeping the map here (rather than
    /// asking a row for its own index) means it is refreshed by exactly the
    /// pass that invalidates it.
    row_nodes: RefCell<Vec<(u64, usize)>>,
    /// Type-ahead search buffer (lowercased) and when it was last extended.
    search: RefCell<String>,
    search_at: Cell<Option<Instant>>,
    /// Bumped on any expand/select change so the row list rebuilds.
    revision: Cell<u64>,
    on_select: RefCell<Option<SelectHandler>>,
}

impl TreeState {
    fn bump(&self) {
        self.revision.set(self.revision.get().wrapping_add(1));
    }

    fn is_expanded(&self, id: u64) -> bool {
        self.expanded.borrow().contains(&id)
    }

    fn toggle(&self, id: u64) {
        let mut set = self.expanded.borrow_mut();
        if !set.insert(id) {
            set.remove(&id);
        }
        drop(set);
        self.bump();
    }

    fn select(&self, id: u64, ctx: &mut EventContext) {
        self.selected.set(Some(id));
        self.bump();
        if let Some(handler) = self.on_select.borrow_mut().as_mut() {
            handler(id, ctx);
        }
    }

    /// Flatten the currently-visible rows, depth-first — the row list the view
    /// builds *and* the order the arrow keys walk, from one definition.
    fn visible_rows(&self) -> Vec<RowInfo> {
        fn walk(
            state: &TreeState,
            items: &[TreeItem],
            depth: usize,
            parent: Option<u64>,
            out: &mut Vec<RowInfo>,
        ) {
            for item in items {
                let expanded = state.is_expanded(item.id);
                out.push(RowInfo {
                    id: item.id,
                    label: item.label.clone(),
                    depth,
                    has_children: item.has_children(),
                    expanded,
                    parent,
                });
                if item.has_children() && expanded {
                    walk(state, &item.children, depth + 1, Some(item.id), out);
                }
            }
        }

        let mut out = Vec::new();
        walk(self, &self.items, 0, None, &mut out);
        out
    }

    /// Where the cursor actually is, resolved against what is on screen right
    /// now: the active row if it is still visible, else the selection, else the
    /// first row. Collapsing a node can hide the active row (a click on its
    /// chevron, with the cursor parked on a descendant), and the cursor must
    /// land somewhere real rather than falling off the list.
    fn cursor(&self, rows: &[RowInfo]) -> Option<u64> {
        let visible = |id: u64| rows.iter().any(|r| r.id == id);
        self.active
            .get()
            .filter(|&id| visible(id))
            .or_else(|| self.selected.get().filter(|&id| visible(id)))
            .or_else(|| rows.first().map(|r| r.id))
    }

    /// Tree index of the row displaying `id`, if it is currently built.
    fn node_of(&self, id: u64) -> Option<usize> {
        self.row_nodes
            .borrow()
            .iter()
            .find(|(row_id, _)| *row_id == id)
            .map(|(_, idx)| *idx)
    }

    /// Move the roving cursor to `id` and scroll that row into view.
    ///
    /// The reveal is why this needs the context: focus itself never moves (it
    /// stays on the host), so without it nothing would follow a cursor that
    /// arrowed off the bottom of a scrolled tree.
    fn move_cursor(&self, id: u64, ctx: &mut EventContext) {
        self.active.set(Some(id));
        if let Some(idx) = self.node_of(id) {
            ctx.reveal(idx);
        }
    }

    /// Drop any in-flight type-ahead search.
    fn clear_search(&self) {
        self.search.borrow_mut().clear();
        self.search_at.set(None);
    }
}

/// A collapsible tree of [`TreeItem`]s. Configure with the builder methods, then
/// [`build`](Self::build) it into the tree.
pub struct TreeView {
    items: Vec<TreeItem>,
    expanded: HashSet<u64>,
    selected: Option<u64>,
    on_select: Option<SelectHandler>,
}

impl TreeView {
    /// A tree over `items`, all collapsed and nothing selected.
    pub fn new(items: Vec<TreeItem>) -> Self {
        Self {
            items,
            expanded: HashSet::new(),
            selected: None,
            on_select: None,
        }
    }

    /// Ids to start expanded.
    pub fn expanded(mut self, ids: impl IntoIterator<Item = u64>) -> Self {
        self.expanded = ids.into_iter().collect();
        self
    }

    /// The initially selected row id.
    pub fn selected(mut self, id: u64) -> Self {
        self.selected = Some(id);
        self
    }

    /// Callback fired whenever a row is selected, receiving its id and the
    /// dispatch [`EventContext`] (so it can queue tree mutations).
    pub fn on_select(mut self, f: impl FnMut(u64, &mut EventContext) + 'static) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }

    /// Mount the tree under `parent`, returning the host node's index.
    ///
    /// The returned node is the tree's tab stop, with the reactive row list as
    /// its only child.
    pub fn build(self, tree: &mut WidgetTree, parent: usize) -> usize {
        let state = Rc::new(TreeState {
            items: self.items,
            expanded: RefCell::new(self.expanded),
            selected: Cell::new(self.selected),
            active: Cell::new(self.selected),
            focused: Cell::new(false),
            host: Cell::new(None),
            row_nodes: RefCell::new(Vec::new()),
            search: RefCell::new(String::new()),
            search_at: Cell::new(None),
            revision: Cell::new(0),
            on_select: RefCell::new(self.on_select),
        });

        let host = tree.add_child(
            parent,
            TreeHost {
                state: Rc::clone(&state),
            },
        );
        state.host.set(Some(host));

        let version_state = Rc::clone(&state);
        let build_state = Rc::clone(&state);
        let reactive = ReactiveChildren::column().width_full().source(
            move || version_state.revision.get(),
            move |tree, node| {
                // The index map describes *this* build only — every row it
                // named is tombstoned by the time we get here.
                build_state.row_nodes.borrow_mut().clear();
                for row in build_state.visible_rows() {
                    let idx = tree.add_child(
                        node,
                        TreeRow {
                            state: Rc::clone(&build_state),
                            id: row.id,
                            label: row.label,
                            depth: row.depth,
                            has_children: row.has_children,
                            expanded: row.expanded,
                            interaction: InteractionState::default(),
                        },
                    );
                    build_state.row_nodes.borrow_mut().push((row.id, idx));
                }
            },
        );
        tree.add_child(host, reactive);
        host
    }
}

/// The tree's single tab stop: an invisible box that owns the row list and,
/// while focused, the keyboard.
///
/// Focus never lands on a row — see the module docs for why that matters when a
/// toggle rebuilds them.
struct TreeHost {
    state: Rc<TreeState>,
}

impl TreeHost {
    /// Handle a named key while focused. `rows` is this frame's visible list and
    /// `at` the cursor's position in it.
    fn named_key(
        &self,
        named: NamedKey,
        rows: &[RowInfo],
        at: usize,
        ctx: &mut EventContext,
    ) -> EventResult {
        let row = &rows[at];
        match named {
            NamedKey::ArrowDown => {
                self.state
                    .move_cursor(rows[step_selection(at, rows.len(), 1)].id, ctx);
            }
            NamedKey::ArrowUp => {
                self.state
                    .move_cursor(rows[step_selection(at, rows.len(), -1)].id, ctx);
            }
            NamedKey::Home => self.state.move_cursor(rows[0].id, ctx),
            NamedKey::End => self.state.move_cursor(rows[rows.len() - 1].id, ctx),
            // → opens a closed node, then descends into an open one. The first
            // child is always the next visible row (the list is depth-first),
            // so no second walk is needed to find it.
            NamedKey::ArrowRight => {
                if row.has_children && !row.expanded {
                    self.state.toggle(row.id);
                } else if row.has_children {
                    if let Some(child) = rows.get(at + 1) {
                        self.state.move_cursor(child.id, ctx);
                    }
                } else {
                    return EventResult::Ignored;
                }
            }
            // ← is the mirror: close an open node, otherwise climb out of it.
            NamedKey::ArrowLeft => {
                if row.has_children && row.expanded {
                    self.state.toggle(row.id);
                } else if let Some(parent) = row.parent {
                    self.state.move_cursor(parent, ctx);
                } else {
                    return EventResult::Ignored;
                }
            }
            NamedKey::Enter => self.state.select(row.id, ctx),
            _ => return EventResult::Ignored,
        }
        // Any accepted navigation ends the current type-ahead search: the next
        // letter starts fresh from wherever the cursor now is.
        self.state.clear_search();
        EventResult::Consumed
    }

    /// Extend the type-ahead search with `ch` and jump to the next match.
    ///
    /// A miss leaves both the cursor and the buffer as they were, so a typo
    /// can't poison the rest of the search.
    fn type_ahead(
        &self,
        ch: char,
        rows: &[RowInfo],
        at: usize,
        ctx: &mut EventContext,
    ) -> EventResult {
        let now = Instant::now();
        let expired = self
            .state
            .search_at
            .get()
            .is_none_or(|last| now.duration_since(last) > TYPE_AHEAD_TIMEOUT);
        let previous = if expired {
            String::new()
        } else {
            self.state.search.borrow().clone()
        };
        let typed: String = ch.to_lowercase().collect();
        // Pressing the same key again *cycles* through the rows starting with
        // that letter rather than searching for a doubled one — the list-box
        // convention, at the documented cost of not being able to type-ahead to
        // a label that really does start "aa".
        let needle = if previous == typed {
            typed
        } else {
            previous + &typed
        };

        // A fresh single letter scans from the row *after* the cursor, so
        // pressing it again cycles to the next match; extending a buffer scans
        // from the cursor itself, so refining a search keeps the row it already
        // found. Both wrap.
        let start = if needle.chars().count() > 1 {
            at
        } else {
            (at + 1) % rows.len()
        };
        let hit = (0..rows.len())
            .map(|step| (start + step) % rows.len())
            .find(|&i| starts_with_ignore_case(&rows[i].label, &needle));

        let Some(hit) = hit else {
            return EventResult::Ignored;
        };
        *self.state.search.borrow_mut() = needle;
        self.state.search_at.set(Some(now));
        self.state.move_cursor(rows[hit].id, ctx);
        EventResult::Consumed
    }
}

/// Whether `label` starts with the already-lowercased `needle`, ignoring case.
fn starts_with_ignore_case(label: &str, needle: &str) -> bool {
    let mut label = label.chars().flat_map(char::to_lowercase);
    needle
        .chars()
        .all(|want| label.next().is_some_and(|got| got == want))
}

impl Widget for TreeHost {
    fn focusable(&self) -> bool {
        true
    }

    fn style(&self) -> FlexStyle {
        FlexStyle::new().column().width_full()
    }

    fn paint(&self, _layout: Rect, _ctx: &mut PaintContext) {}

    fn event(&mut self, event: &WidgetEvent, _layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::FocusGained => {
                self.state.focused.set(true);
                // Park the cursor on entry so the first arrow key has somewhere
                // to move from, and show where that is.
                let rows = self.state.visible_rows();
                if let Some(id) = self.state.cursor(&rows) {
                    self.state.move_cursor(id, ctx);
                }
                EventResult::Ignored
            }
            WidgetEvent::FocusLost => {
                self.state.focused.set(false);
                self.state.clear_search();
                EventResult::Ignored
            }
            _ if !self.state.focused.get() => EventResult::Ignored,
            WidgetEvent::KeyDown {
                key: Key::Named(named),
            } => {
                let rows = self.state.visible_rows();
                let Some(at) = self
                    .state
                    .cursor(&rows)
                    .and_then(|id| rows.iter().position(|r| r.id == id))
                else {
                    return EventResult::Ignored;
                };
                self.named_key(*named, &rows, at, ctx)
            }
            // A bare Space arrives as `CharInput { ' ' }` (winit routes it
            // through the character pipeline — Button and Checkbox share this
            // path), so activation and type-ahead meet here. Space commits
            // unless it is continuing a search, where it is a real character.
            WidgetEvent::CharInput { ch } => {
                let rows = self.state.visible_rows();
                let Some(at) = self
                    .state
                    .cursor(&rows)
                    .and_then(|id| rows.iter().position(|r| r.id == id))
                else {
                    return EventResult::Ignored;
                };
                if *ch == ' ' && self.state.search.borrow().is_empty() {
                    self.state.select(rows[at].id, ctx);
                    return EventResult::Consumed;
                }
                if ch.is_control() {
                    return EventResult::Ignored;
                }
                self.type_ahead(*ch, &rows, at, ctx)
            }
            _ => EventResult::Ignored,
        }
    }
}

/// One self-painting row: chevron (if any), label, and selection / hover
/// background, all read from the active theme.
struct TreeRow {
    state: Rc<TreeState>,
    id: u64,
    label: String,
    depth: usize,
    has_children: bool,
    expanded: bool,
    interaction: InteractionState,
}

impl TreeRow {
    /// The row-local x where this row's chevron slot begins.
    fn chevron_x(&self, layout: Rect) -> f32 {
        layout.origin.x + LEFT_PAD + self.depth as f32 * INDENT
    }
}

impl Widget for TreeRow {
    fn style(&self) -> FlexStyle {
        FlexStyle::new().width_full().height(ROW_HEIGHT)
    }

    fn paint(&self, layout: Rect, ctx: &mut PaintContext) {
        let selected = self.state.selected.get() == Some(self.id);

        // Selection wins over hover; both are rounded fills inset a hair so the
        // highlight reads as a pill, not a full-bleed band.
        let inset = Rect::new(
            layout.origin.x + 2.0,
            layout.origin.y + 1.0,
            (layout.size.width - 4.0).max(0.0),
            (layout.size.height - 2.0).max(0.0),
        );
        if selected {
            let mut c = ctx.theme.colors.primary;
            c = Color::rgba(c.r, c.g, c.b, 0.20);
            ctx.fill_rect_rounded(inset, c, ROW_RADIUS);
        } else if self.interaction.hovered {
            ctx.fill_rect_rounded(inset, ctx.theme.hover.bg, ROW_RADIUS);
        }

        // The roving cursor rides the focus ring: it marks where the arrow keys
        // move from, which is only meaningful while the tree owns the keyboard.
        if self.state.focused.get()
            && self.state.active.get() == Some(self.id)
            && ctx.focus_visible()
        {
            ctx.paint_focus_ring(inset, None, ROW_RADIUS);
        }

        let font = ctx.theme.typography.body.font_size;
        let line = ctx.theme.typography.body.line_height;
        let attrs = TextAttrs::default();
        let mid_y = layout.origin.y + layout.size.height / 2.0;
        let chevron_x = self.chevron_x(layout);

        // Disclosure chevron for parents: ▾ when open, ▸ when closed.
        if self.has_children {
            let glyph = if self.expanded {
                "\u{25BE}"
            } else {
                "\u{25B8}"
            };
            let shaped = ctx
                .text_engine
                .shape_text_attrs(glyph, font, line, None, &attrs);
            let gx = chevron_x + (CHEVRON_W - shaped.width) / 2.0;
            let gy = mid_y - shaped.height / 2.0;
            let color = ctx.theme.colors.on_surface_variant;
            for g in &shaped.glyphs {
                if let Some(img) = ctx.text_engine.rasterize(g.cache_key) {
                    ctx.draw_glyph(gx + g.x, gy + g.y, img, color, g.cache_key);
                }
            }
        }

        // Label, to the right of the chevron slot.
        let label_x = chevron_x + CHEVRON_W + LABEL_GAP;
        let shaped = ctx
            .text_engine
            .shape_text_attrs(&self.label, font, line, None, &attrs);
        let ly = mid_y - shaped.height / 2.0;
        let color = ctx.theme.colors.on_surface;
        ctx.push_clip(layout);
        for g in &shaped.glyphs {
            if let Some(img) = ctx.text_engine.rasterize(g.cache_key) {
                ctx.draw_glyph(label_x + g.x, ly + g.y, img, color, g.cache_key);
            }
        }
        ctx.pop_clip();
    }

    fn event(&mut self, event: &WidgetEvent, layout: Rect, ctx: &mut EventContext) -> EventResult {
        match event {
            WidgetEvent::MouseEnter => {
                self.interaction.enter(false);
                EventResult::Consumed
            }
            WidgetEvent::MouseLeave => {
                self.interaction.leave();
                EventResult::Consumed
            }
            WidgetEvent::MouseDown {
                button: MouseButton::Left,
                position,
            } => {
                // Inside the chevron slot on a parent → toggle; anywhere else on
                // the row → select. Two zones, one hit target, no nested clicks.
                let cx = self.chevron_x(layout);
                let on_chevron =
                    self.has_children && position.x >= cx && position.x < cx + CHEVRON_W;
                if on_chevron {
                    self.state.toggle(self.id);
                } else {
                    self.state.select(self.id, ctx);
                }
                // A click also parks the cursor here and hands the keyboard to
                // the host, so the arrows carry on from the row just touched.
                // (Built-in click-to-focus can't do it: it only ever focuses the
                // widget actually hit, and a row is deliberately not a tab stop.)
                self.state.active.set(Some(self.id));
                self.state.clear_search();
                if let Some(host) = self.state.host.get() {
                    ctx.focus(host);
                    // ⚠ That focus arms a scroll-into-view for the *host* — the
                    // whole tree — which in a scrolled viewport would yank the
                    // list under the cursor mid-click. Queue this row's reveal
                    // behind it: the last reveal of a dispatch wins, and
                    // revealing a row you just clicked (so, a visible one) moves
                    // nothing.
                    if let Some(idx) = self.state.node_of(self.id) {
                        ctx.reveal(idx);
                    }
                }
                EventResult::Consumed
            }
            _ => EventResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Vec<TreeItem> {
        vec![
            TreeItem::new(1, "src")
                .child(TreeItem::new(2, "main.rs"))
                .child(TreeItem::with_children(
                    3,
                    "widgets",
                    vec![TreeItem::new(4, "button.rs")],
                )),
            TreeItem::new(5, "README.md"),
        ]
    }

    /// A flat forest with two `a` labels — the shape a type-ahead cycle needs.
    fn flat_model() -> Vec<TreeItem> {
        vec![
            TreeItem::new(1, "alpha"),
            TreeItem::new(2, "apple"),
            TreeItem::new(3, "Beta"),
        ]
    }

    fn state_with(items: Vec<TreeItem>, expanded: &[u64]) -> Rc<TreeState> {
        Rc::new(TreeState {
            items,
            expanded: RefCell::new(expanded.iter().copied().collect()),
            selected: Cell::new(None),
            active: Cell::new(None),
            focused: Cell::new(false),
            host: Cell::new(None),
            row_nodes: RefCell::new(Vec::new()),
            search: RefCell::new(String::new()),
            search_at: Cell::new(None),
            revision: Cell::new(0),
            on_select: RefCell::new(None),
        })
    }

    fn state(expanded: &[u64]) -> Rc<TreeState> {
        state_with(model(), expanded)
    }

    /// A host that has already taken focus — the precondition for every key.
    fn focused_host(state: &Rc<TreeState>) -> TreeHost {
        let mut host = TreeHost {
            state: Rc::clone(state),
        };
        host.event(
            &WidgetEvent::FocusGained,
            layout(),
            &mut EventContext::new(),
        );
        host
    }

    fn press(host: &mut TreeHost, named: NamedKey) -> EventResult {
        host.event(
            &WidgetEvent::KeyDown {
                key: Key::Named(named),
            },
            layout(),
            &mut EventContext::new(),
        )
    }

    /// Type `text` one `CharInput` at a time, returning the last result.
    fn type_text(host: &mut TreeHost, text: &str) -> EventResult {
        let mut result = EventResult::Ignored;
        for ch in text.chars() {
            result = host.event(
                &WidgetEvent::CharInput { ch },
                layout(),
                &mut EventContext::new(),
            );
        }
        result
    }

    /// Ids of the currently-visible rows, in display order.
    fn visible_ids(state: &Rc<TreeState>) -> Vec<u64> {
        state.visible_rows().into_iter().map(|r| r.id).collect()
    }

    fn row(
        state: &Rc<TreeState>,
        id: u64,
        depth: usize,
        has_children: bool,
        expanded: bool,
    ) -> TreeRow {
        TreeRow {
            state: Rc::clone(state),
            id,
            label: String::new(),
            depth,
            has_children,
            expanded,
            interaction: InteractionState::default(),
        }
    }

    /// A 300px row starting at x=0; chevron slot for depth `d` is
    /// [LEFT_PAD + d*INDENT, +CHEVRON_W].
    fn layout() -> Rect {
        Rect::new(0.0, 0.0, 300.0, ROW_HEIGHT)
    }

    fn down(x: f32) -> WidgetEvent {
        WidgetEvent::MouseDown {
            button: MouseButton::Left,
            position: shroud_core::Point::new(x, 12.0),
        }
    }

    #[test]
    fn toggle_flips_expansion_and_bumps_the_revision() {
        let st = state(&[]);
        assert!(!st.is_expanded(1));
        let rev = st.revision.get();
        st.toggle(1);
        assert!(st.is_expanded(1), "first toggle expands");
        assert_ne!(st.revision.get(), rev, "a toggle bumps the rebuild token");
        st.toggle(1);
        assert!(!st.is_expanded(1), "second toggle collapses");
    }

    #[test]
    fn clicking_the_chevron_zone_toggles() {
        let st = state(&[]);
        // depth 0 chevron slot starts at LEFT_PAD (8) and is CHEVRON_W (16) wide.
        let mut r = row(&st, 1, 0, true, false);
        let mut ctx = EventContext::new();
        r.event(&down(LEFT_PAD + 4.0), layout(), &mut ctx);
        assert!(st.is_expanded(1), "a click in the chevron zone expands");
        assert_eq!(st.selected.get(), None, "and does not select");
    }

    #[test]
    fn clicking_the_label_zone_selects() {
        let st = state(&[]);
        let mut r = row(&st, 1, 0, true, false);
        let mut ctx = EventContext::new();
        // Well past the chevron slot → the label zone.
        r.event(&down(120.0), layout(), &mut ctx);
        assert_eq!(
            st.selected.get(),
            Some(1),
            "a click on the row body selects"
        );
        assert!(!st.is_expanded(1), "and does not toggle");
    }

    #[test]
    fn leaf_row_selects_even_in_the_chevron_zone() {
        // A leaf has no chevron, so the whole row selects.
        let st = state(&[]);
        let mut r = row(&st, 5, 0, false, false);
        let mut ctx = EventContext::new();
        r.event(&down(LEFT_PAD + 4.0), layout(), &mut ctx);
        assert_eq!(st.selected.get(), Some(5));
    }

    #[test]
    fn visible_rows_flattens_only_visible_descendants() {
        // Collapsed: just the two roots.
        assert_eq!(visible_ids(&state(&[])), vec![1, 5], "all collapsed");
        // Expand `src` (1): its two children show, but `widgets` (3) stays closed.
        assert_eq!(
            visible_ids(&state(&[1])),
            vec![1, 2, 3, 5],
            "src open → src, main.rs, widgets, README"
        );
        // Expand `widgets` (3) too: its child appears.
        assert_eq!(
            visible_ids(&state(&[1, 3])),
            vec![1, 2, 3, 4, 5],
            "widgets open → its child shows too"
        );
    }

    #[test]
    fn visible_rows_carry_depth_and_parent() {
        let rows = state(&[1, 3]).visible_rows();
        let by_id = |id: u64| rows.iter().find(|r| r.id == id).expect("row present");
        assert_eq!(
            (by_id(1).depth, by_id(1).parent),
            (0, None),
            "src is a root"
        );
        assert_eq!(
            (by_id(2).depth, by_id(2).parent),
            (1, Some(1)),
            "main.rs sits under src"
        );
        assert_eq!(
            (by_id(4).depth, by_id(4).parent),
            (2, Some(3)),
            "button.rs sits under widgets"
        );
        assert!(
            by_id(3).has_children && by_id(3).expanded,
            "widgets is open"
        );
        assert!(!by_id(5).has_children, "README is a leaf");
    }

    #[test]
    fn arrows_walk_the_visible_rows_and_clamp() {
        let st = state(&[1]); // rows: 1, 2, 3, 5
        let mut host = focused_host(&st);
        assert_eq!(st.active.get(), Some(1), "focus parks on the first row");

        press(&mut host, NamedKey::ArrowDown);
        assert_eq!(st.active.get(), Some(2));
        press(&mut host, NamedKey::ArrowDown);
        press(&mut host, NamedKey::ArrowDown);
        assert_eq!(st.active.get(), Some(5), "walked to the last row");
        press(&mut host, NamedKey::ArrowDown);
        assert_eq!(st.active.get(), Some(5), "the end clamps, it does not wrap");

        press(&mut host, NamedKey::ArrowUp);
        assert_eq!(st.active.get(), Some(3));
        for _ in 0..5 {
            press(&mut host, NamedKey::ArrowUp);
        }
        assert_eq!(st.active.get(), Some(1), "the top clamps too");
    }

    #[test]
    fn home_and_end_jump_to_the_ends() {
        let st = state(&[1]);
        let mut host = focused_host(&st);
        press(&mut host, NamedKey::End);
        assert_eq!(st.active.get(), Some(5));
        press(&mut host, NamedKey::Home);
        assert_eq!(st.active.get(), Some(1));
    }

    #[test]
    fn arrows_move_the_cursor_without_selecting() {
        // The decision this widget is built on: walking a tree must not fire
        // `on_select`, so a handler that loads something expensive stays idle
        // until the user commits.
        let st = state(&[1]);
        let mut host = focused_host(&st);
        press(&mut host, NamedKey::ArrowDown);
        press(&mut host, NamedKey::ArrowDown);
        assert_eq!(st.active.get(), Some(3), "the cursor moved");
        assert_eq!(st.selected.get(), None, "and the selection did not follow");
    }

    #[test]
    fn enter_and_space_commit_the_cursor_row() {
        let st = state(&[1]);
        let mut host = focused_host(&st);
        press(&mut host, NamedKey::ArrowDown);
        press(&mut host, NamedKey::Enter);
        assert_eq!(st.selected.get(), Some(2), "Enter selects the cursor row");

        press(&mut host, NamedKey::ArrowDown);
        // Space reaches a widget as a `CharInput`, not a `KeyDown`.
        type_text(&mut host, " ");
        assert_eq!(st.selected.get(), Some(3), "Space selects too");
    }

    #[test]
    fn right_opens_a_closed_node_then_descends_into_it() {
        let st = state(&[]); // rows: 1, 5 — everything closed
        let mut host = focused_host(&st);
        assert_eq!(st.active.get(), Some(1));

        press(&mut host, NamedKey::ArrowRight);
        assert!(st.is_expanded(1), "the first press opens the node");
        assert_eq!(st.active.get(), Some(1), "and leaves the cursor on it");

        press(&mut host, NamedKey::ArrowRight);
        assert_eq!(st.active.get(), Some(2), "the second press enters the node");

        // On a leaf there is nowhere to go.
        press(&mut host, NamedKey::ArrowRight);
        assert_eq!(st.active.get(), Some(2), "a leaf ignores →");
    }

    #[test]
    fn left_closes_an_open_node_then_climbs_out_of_it() {
        let st = state(&[1, 3]); // rows: 1, 2, 3, 4, 5
        let mut host = focused_host(&st);
        press(&mut host, NamedKey::ArrowDown); // → main.rs (a leaf under src)

        press(&mut host, NamedKey::ArrowLeft);
        assert_eq!(st.active.get(), Some(1), "a leaf climbs to its parent");
        assert!(st.is_expanded(1), "climbing does not close anything");

        press(&mut host, NamedKey::ArrowLeft);
        assert!(!st.is_expanded(1), "an open node closes under the cursor");
        assert_eq!(st.active.get(), Some(1), "and the cursor stays on it");

        // A closed root has no parent to climb to.
        assert_eq!(press(&mut host, NamedKey::ArrowLeft), EventResult::Ignored);
        assert_eq!(st.active.get(), Some(1));
    }

    #[test]
    fn the_cursor_survives_a_collapse_that_hides_it() {
        // Collapsing by mouse can hide the active row; the cursor must land on
        // something real rather than falling off the list.
        let st = state(&[1, 3]);
        let mut host = focused_host(&st);
        press(&mut host, NamedKey::End); // README, still visible
        st.active.set(Some(4)); // park on button.rs, deep inside `widgets`
        st.expanded.borrow_mut().remove(&1); // collapse src by hand (a click)

        let rows = st.visible_rows();
        assert_eq!(st.cursor(&rows), Some(1), "falls back to a visible row");
        press(&mut host, NamedKey::ArrowDown);
        assert_eq!(
            st.active.get(),
            Some(5),
            "and navigation carries on from it"
        );
    }

    #[test]
    fn type_ahead_jumps_to_the_next_match_and_cycles() {
        let st = state_with(flat_model(), &[]); // alpha, apple, Beta
        let mut host = focused_host(&st);
        assert_eq!(st.active.get(), Some(1), "cursor starts on alpha");

        // A fresh letter scans from *after* the cursor, so `a` on alpha finds
        // apple; pressing it again cycles back round to alpha.
        type_text(&mut host, "a");
        assert_eq!(st.active.get(), Some(2), "jumped to apple");
        type_text(&mut host, "a");
        assert_eq!(st.active.get(), Some(1), "repeat cycles to the next match");

        // A *different* letter inside the timeout extends the search instead of
        // starting one — "ab" matches nothing here, so nothing moves. (Starting
        // fresh after the timeout is `a_stale_search_buffer_expires`.)
        assert_eq!(type_text(&mut host, "b"), EventResult::Ignored);
        assert_eq!(st.active.get(), Some(1));
    }

    #[test]
    fn type_ahead_extends_the_buffer_to_refine() {
        let st = state_with(flat_model(), &[]);
        let mut host = focused_host(&st);
        // `ap` must land on apple, not stop at alpha: the second letter extends
        // the search rather than starting a new one.
        type_text(&mut host, "ap");
        assert_eq!(st.active.get(), Some(2));
        assert_eq!(&*st.search.borrow(), "ap");
    }

    #[test]
    fn a_type_ahead_miss_changes_nothing() {
        let st = state_with(flat_model(), &[]);
        let mut host = focused_host(&st);
        type_text(&mut host, "a");
        assert_eq!(st.active.get(), Some(2));

        assert_eq!(
            type_text(&mut host, "z"),
            EventResult::Ignored,
            "no row starts with `az`"
        );
        assert_eq!(st.active.get(), Some(2), "the cursor stayed put");
        assert_eq!(
            &*st.search.borrow(),
            "a",
            "and the typo did not poison the buffer"
        );
    }

    #[test]
    fn a_stale_search_buffer_expires() {
        let st = state_with(flat_model(), &[]);
        let mut host = focused_host(&st);
        type_text(&mut host, "a"); // → apple, buffer "a"

        // Backdate the buffer past the timeout: the next letter must start a
        // fresh search instead of extending it into "ab".
        st.search_at.set(Some(
            Instant::now() - TYPE_AHEAD_TIMEOUT - Duration::from_millis(1),
        ));
        type_text(&mut host, "b");
        assert_eq!(st.active.get(), Some(3), "`b` alone found Beta");
        assert_eq!(&*st.search.borrow(), "b");
    }

    #[test]
    fn navigation_ends_the_search() {
        let st = state_with(flat_model(), &[]);
        let mut host = focused_host(&st);
        type_text(&mut host, "a");
        press(&mut host, NamedKey::ArrowDown);
        assert!(
            st.search.borrow().is_empty(),
            "an arrow key starts the next search fresh"
        );
        // ...so Space commits rather than being treated as a search character.
        type_text(&mut host, " ");
        assert_eq!(st.selected.get(), st.active.get());
    }

    #[test]
    fn keys_are_inert_until_the_tree_has_focus() {
        let st = state(&[1]);
        let mut host = TreeHost {
            state: Rc::clone(&st),
        };
        assert_eq!(press(&mut host, NamedKey::ArrowDown), EventResult::Ignored);
        assert_eq!(st.active.get(), None, "no cursor without focus");

        host.event(
            &WidgetEvent::FocusGained,
            layout(),
            &mut EventContext::new(),
        );
        press(&mut host, NamedKey::ArrowDown);
        assert_eq!(st.active.get(), Some(2));

        host.event(&WidgetEvent::FocusLost, layout(), &mut EventContext::new());
        assert_eq!(press(&mut host, NamedKey::ArrowDown), EventResult::Ignored);
        assert_eq!(st.active.get(), Some(2), "the cursor is kept, not moved");
    }

    #[test]
    fn focus_parks_the_cursor_on_the_selection() {
        let st = state(&[1]);
        st.selected.set(Some(3));
        let _host = focused_host(&st);
        assert_eq!(
            st.active.get(),
            Some(3),
            "entering the tree resumes at the selected row"
        );
    }

    #[test]
    fn clicking_a_row_parks_the_cursor_and_takes_focus() {
        let st = state(&[]);
        st.host.set(Some(7));
        let mut r = row(&st, 5, 0, false, false);
        let mut ctx = EventContext::new();
        r.event(&down(120.0), layout(), &mut ctx);
        assert_eq!(st.active.get(), Some(5), "the cursor follows the click");
        assert!(
            ctx.commands
                .iter()
                .any(|c| matches!(c, crate::event::TreeCommand::Focus { target: Some(7) })),
            "and the keyboard is handed to the host"
        );
    }

    #[test]
    fn select_fires_the_handler_with_the_id() {
        use std::rc::Rc as StdRc;
        let seen = StdRc::new(Cell::new(0u64));
        let seen2 = StdRc::clone(&seen);
        let st = state(&[]);
        *st.on_select.borrow_mut() = Some(Box::new(move |id, _ctx| seen2.set(id)));
        let mut ctx = EventContext::new();
        st.select(4, &mut ctx);
        assert_eq!(seen.get(), 4, "the select handler saw the row id");
    }
}
