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
//! This is a mouse-driven MVP: expand / collapse / select by clicking. Keyboard
//! roving (arrow-key navigation, type-ahead) and OS a11y tree exposure are
//! deliberate follow-ups — the [`AccessRole`](shroud_core::AccessRole)
//! vocabulary has no tree role yet.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use crate::event::{EventContext, EventResult, MouseButton, WidgetEvent};
use crate::interaction::InteractionState;
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

/// State shared by the row builder and every row widget: the model, the
/// expansion / selection sets, a revision the [`ReactiveChildren`] watches, and
/// the selection callback.
struct TreeState {
    items: Vec<TreeItem>,
    expanded: RefCell<HashSet<u64>>,
    selected: Cell<Option<u64>>,
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

    /// Mount the tree under `parent`, returning the container node's index.
    pub fn build(self, tree: &mut WidgetTree, parent: usize) -> usize {
        let state = Rc::new(TreeState {
            items: self.items,
            expanded: RefCell::new(self.expanded),
            selected: Cell::new(self.selected),
            revision: Cell::new(0),
            on_select: RefCell::new(self.on_select),
        });

        let version_state = Rc::clone(&state);
        let build_state = Rc::clone(&state);
        let reactive = ReactiveChildren::column().width_full().source(
            move || version_state.revision.get(),
            move |tree, node| {
                let items = &build_state.items;
                for item in items {
                    add_rows(&build_state, tree, node, item, 0);
                }
            },
        );
        tree.add_child(parent, reactive)
    }
}

/// Append `item`'s row and, if it is expanded, its descendants' rows — a
/// depth-first flatten of the currently-visible subtree.
fn add_rows(
    state: &Rc<TreeState>,
    tree: &mut WidgetTree,
    parent: usize,
    item: &TreeItem,
    depth: usize,
) {
    let expanded = state.is_expanded(item.id);
    tree.add_child(
        parent,
        TreeRow {
            state: Rc::clone(state),
            id: item.id,
            label: item.label.clone(),
            depth,
            has_children: item.has_children(),
            expanded,
            interaction: InteractionState::default(),
        },
    );
    if item.has_children() && expanded {
        for child in &item.children {
            add_rows(state, tree, parent, child, depth + 1);
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

    fn state(expanded: &[u64]) -> Rc<TreeState> {
        Rc::new(TreeState {
            items: model(),
            expanded: RefCell::new(expanded.iter().copied().collect()),
            selected: Cell::new(None),
            revision: Cell::new(0),
            on_select: RefCell::new(None),
        })
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
    fn add_rows_flattens_only_visible_descendants() {
        // Count rows a build would emit by walking the same logic.
        fn count(state: &Rc<TreeState>, item: &TreeItem) -> usize {
            let mut n = 1;
            if item.has_children() && state.is_expanded(item.id) {
                for c in &item.children {
                    n += count(state, c);
                }
            }
            n
        }

        // Collapsed: just the two roots.
        let st = state(&[]);
        let total: usize = st.items.iter().map(|i| count(&st, i)).sum();
        assert_eq!(total, 2, "all collapsed → two root rows");

        // Expand `src` (1): its two children show, but `widgets` (3) stays closed.
        let st = state(&[1]);
        let total: usize = st.items.iter().map(|i| count(&st, i)).sum();
        assert_eq!(total, 4, "src open → src, main.rs, widgets, README");

        // Expand `widgets` (3) too: its child appears.
        let st = state(&[1, 3]);
        let total: usize = st.items.iter().map(|i| count(&st, i)).sum();
        assert_eq!(total, 5, "widgets open → its child shows too");
    }

    #[test]
    fn select_fires_the_handler_with_the_id() {
        use std::rc::Rc as StdRc;
        let seen = StdRc::new(Cell::new(0u64));
        let seen2 = StdRc::clone(&seen);
        let st = Rc::new(TreeState {
            items: model(),
            expanded: RefCell::new(HashSet::new()),
            selected: Cell::new(None),
            revision: Cell::new(0),
            on_select: RefCell::new(Some(Box::new(move |id, _ctx| seen2.set(id)))),
        });
        let mut ctx = EventContext::new();
        st.select(4, &mut ctx);
        assert_eq!(seen.get(), 4, "the select handler saw the row id");
    }
}
