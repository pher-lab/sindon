//! VirtualList — a fixed-row-height list that materializes only the rows in (or
//! near) the viewport, so a list of thousands of items lays out a screenful of
//! widgets instead of all of them.
//!
//! It is a thin windowing layer over [`ScrollView`](crate::ScrollView): place a
//! `VirtualList` as the child of a `ScrollView` and the tree drives it once per
//! layout pass (see `WidgetTree::sync_virtual_lists`). Each pass it reads the
//! scroll offset + viewport from the enclosing `ScrollView`, pins that scroll
//! view's content height to the full logical extent (item count × row height),
//! computes the visible integer row range, and rebuilds the row subtree only
//! when that range leaves the previously-built (overscanned) window. The
//! `ScrollView` keeps owning scrolling, easing, clipping, and the scrollbar; the
//! `VirtualList` only decides *which* rows exist.
//!
//! Rows are a fixed height — `row_height` is the full row pitch, so put any
//! visual gap inside the row widget. Variable-height virtualization (measuring
//! each row) is a separate, larger effort; uniform rows cover the vault / table
//! shape this was built for.
//!
//! ```ignore
//! let sv = tree.add_child(root, ScrollView::new().width_full().grow(1.0));
//! VirtualList::new(44.0)
//!     .items({ let s = state.clone(); move || s.borrow().len() })
//!     .on_row(move |tree, parent, i| {
//!         tree.add_child(parent, /* one row widget for item `i` */);
//!     })
//!     .build(&mut tree, sv);
//! ```

use std::cell::{Cell, RefCell};

use crate::paint::PaintContext;
use crate::tree::WidgetTree;
use crate::widget::Widget;
use shroud_core::Rect;
use shroud_layout::FlexStyle;

/// Item count. Read once per layout pass.
type CountFn = Box<dyn Fn() -> usize>;

/// Content version. Bumped by the app when the data behind already-built rows
/// changes (not just the count), forcing a rebuild even if the visible range is
/// unchanged. Defaults to a constant `0` (rows never change once built).
type VersionFn = Box<dyn Fn() -> u64>;

/// Builds one row: `|tree, parent, index|` adds a single row widget for `index`
/// under `parent` (the `VirtualList`'s node). `FnMut` because it runs again
/// every time the window changes.
type RowFn = Box<dyn FnMut(&mut WidgetTree, usize, usize)>;

/// Rows kept materialized above and below the viewport so a small scroll doesn't
/// trigger a rebuild.
const DEFAULT_OVERSCAN: usize = 4;

/// The last-built window: `(first, last, data_version, item_count)`. The tree
/// skips rebuilding while the current visible range stays inside `first..last`
/// and neither the version nor the count changed.
type BuiltWindow = (usize, usize, u64, usize);

pub struct VirtualList {
    style: FlexStyle,
    row_height: f32,
    overscan: usize,
    count: CountFn,
    version: VersionFn,
    build_row: RefCell<Option<RowFn>>,
    last_window: Cell<Option<BuiltWindow>>,
}

impl VirtualList {
    /// Create a list whose rows are each `row_height` pixels tall (the full row
    /// pitch). Column layout, full width by default.
    pub fn new(row_height: f32) -> Self {
        Self {
            style: FlexStyle::new().column().width_full(),
            row_height: row_height.max(1.0),
            overscan: DEFAULT_OVERSCAN,
            count: Box::new(|| 0),
            version: Box::new(|| 0),
            build_row: RefCell::new(None),
            last_window: Cell::new(None),
        }
    }

    /// Set the item-count source (read once per layout pass).
    pub fn items<F>(mut self, count: F) -> Self
    where
        F: Fn() -> usize + 'static,
    {
        self.count = Box::new(count);
        self
    }

    /// Set the content-version source. Bump its result when the data behind
    /// already-built rows changes so the visible rows rebuild even though the
    /// range is unchanged. Omit for immutable data.
    pub fn data_version<F>(mut self, version: F) -> Self
    where
        F: Fn() -> u64 + 'static,
    {
        self.version = Box::new(version);
        self
    }

    /// Extra rows kept materialized above and below the viewport so a small
    /// scroll doesn't rebuild. Defaults to [`DEFAULT_OVERSCAN`].
    pub fn overscan(mut self, rows: usize) -> Self {
        self.overscan = rows;
        self
    }

    /// The per-row builder: `|tree, parent, index|` adds one row widget for
    /// `index` under `parent` (the `VirtualList`'s node).
    pub fn on_row<F>(mut self, build: F) -> Self
    where
        F: FnMut(&mut WidgetTree, usize, usize) + 'static,
    {
        self.build_row = RefCell::new(Some(Box::new(build)));
        self
    }

    /// Set padding on all sides.
    pub fn padding(mut self, px: f32) -> Self {
        self.style = self.style.padding(px);
        self
    }

    /// Add this list as a child of `parent` (a [`ScrollView`](crate::ScrollView),
    /// or a container nested within one) and return its node index.
    pub fn build(self, tree: &mut WidgetTree, parent: usize) -> usize {
        tree.add_child(parent, self)
    }

    // ── Tree-facing internals (crate-private) ───────────────────────────────

    /// Full row pitch in pixels.
    pub(crate) fn row_height(&self) -> f32 {
        self.row_height
    }

    /// Overscan row count (above and below the viewport).
    pub(crate) fn overscan_rows(&self) -> usize {
        self.overscan
    }

    /// Current item count.
    pub(crate) fn item_count(&self) -> usize {
        (self.count)()
    }

    /// Current content version (crate accessor; the public builder of the same
    /// concept is [`Self::data_version`]).
    pub(crate) fn content_version(&self) -> u64 {
        (self.version)()
    }

    /// The window the rows were last built for, or `None` before the first build.
    pub(crate) fn last_window(&self) -> Option<BuiltWindow> {
        self.last_window.get()
    }

    /// Record the window the rows were just (re)built for.
    pub(crate) fn set_last_window(&self, w: BuiltWindow) {
        self.last_window.set(Some(w));
    }

    /// Take the row builder out so the tree can call it with `&mut WidgetTree`.
    pub(crate) fn take_builder(&self) -> Option<RowFn> {
        self.build_row.borrow_mut().take()
    }

    /// Return the row builder after a rebuild.
    pub(crate) fn restore_builder(&self, build: RowFn) {
        *self.build_row.borrow_mut() = Some(build);
    }
}

impl Widget for VirtualList {
    fn style(&self) -> FlexStyle {
        self.style.clone()
    }

    /// Pure windowing container — nothing of its own to draw. The spacer and
    /// rows are painted by the tree.
    fn paint(&self, _layout: Rect, _ctx: &mut PaintContext) {}
}
