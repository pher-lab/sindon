//! Paint-time clip culling: what a `ScrollView` scrolls out of view costs
//! nothing to keep off screen.
//!
//! Two independent mechanisms, tested here as one behavior because they cover
//! for each other:
//!
//! 1. `PaintContext` drops any draw command whose quad falls entirely outside
//!    the active clip. Exact (it is the converse of the renderer's scissor),
//!    and the thing that actually bounds the shared glyph atlas — which has no
//!    per-frame limit, so a document taller than its viewport used to pile
//!    every glyph it contains into the atlas and eventually blank new ones.
//! 2. `WidgetTree::paint` skips the `paint` of a widget that sits outside the
//!    clip by a margin, so a `TextWidget` off screen never shapes its string in
//!    the first place. This is where a long markdown preview stops being O(the
//!    whole document) per frame.
//!
//! Together they give the markdown preview the property `VirtualList` gives a
//! long list: per-frame cost tracks the *viewport*, not the content.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use shroud_core::{Color, Point, Rect, Theme};
use shroud_layout::FlexStyle;
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input, ScrollView, TextWidget, Widget};

const VIEW_W: f32 = 400.0;
const VIEW_H: f32 = 300.0;

/// A leaf that records how many times it was asked to paint. Stands in for any
/// widget whose `paint` does real work (shaping, rasterizing) before it emits
/// its first draw command.
struct PaintCounter {
    painted: Rc<Cell<usize>>,
    height: f32,
}

impl PaintCounter {
    fn new(height: f32) -> (Self, Rc<Cell<usize>>) {
        let painted = Rc::new(Cell::new(0));
        (
            Self {
                painted: Rc::clone(&painted),
                height,
            },
            painted,
        )
    }
}

impl Widget for PaintCounter {
    fn style(&self) -> FlexStyle {
        FlexStyle::new().width(VIEW_W).height(self.height)
    }

    fn paint(&self, _layout: Rect, _ctx: &mut PaintContext) {
        self.painted.set(self.painted.get() + 1);
    }
}

fn paint(tree: &WidgetTree) -> PaintContext {
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
    ctx
}

/// Scroll the view under the cursor by `dy` content pixels (wheel deltas are
/// negated: a negative delta scrolls the content up).
fn scroll(tree: &mut WidgetTree, dy: f32) {
    let mut ectx = EventContext::new();
    tree.dispatch_event(
        &WidgetEvent::Scroll {
            position: Point::new(VIEW_W / 2.0, VIEW_H / 2.0),
            delta_x: 0.0,
            delta_y: -dy,
        },
        &mut ectx,
    );
}

/// A `ScrollView` filling the viewport, with instant scrolling so a single
/// dispatch lands the offset the assertions read (the eased offset is what
/// paint consults — see `VirtualList`'s tests for the same discipline).
fn scroll_view(tree: &mut WidgetTree) -> usize {
    tree.set_root(
        ScrollView::new()
            .width(VIEW_W)
            .height(VIEW_H)
            .scroll_transition(Duration::ZERO),
    )
}

#[test]
fn offscreen_scroll_view_children_are_culled_from_the_draw_list() {
    // 100 rows of 40px = 4000px of content in a 300px viewport. Only the rows
    // straddling the viewport may reach the renderer; the rest would be
    // scissored to nothing anyway.
    let mut tree = WidgetTree::new();
    let root = scroll_view(&mut tree);
    let content = tree.add_child(root, Container::column());
    let mark = Color::rgb(1.0, 0.0, 0.0);
    for _ in 0..100 {
        tree.add_child(
            content,
            Container::row().width(VIEW_W).height(40.0).background(mark),
        );
    }
    tree.compute_layout(VIEW_W, VIEW_H);

    // `DrawRect` is a plain renderer command struct (no `Clone`), so keep just
    // the geometry the assertions need.
    let drawn = |ctx: &PaintContext| -> Vec<Rect> {
        ctx.rects
            .iter()
            .filter(|r| r.color == mark)
            .map(|r| Rect::new(r.x, r.y, r.width, r.height))
            .collect()
    };

    let ctx = paint(&tree);
    let rows = drawn(&ctx);
    assert!(
        !rows.is_empty() && rows.len() <= 10,
        "a 300px viewport shows ~8 of 100 rows, but {} were recorded",
        rows.len()
    );

    // Every recorded row genuinely overlaps the viewport.
    let viewport = Rect::new(0.0, 0.0, VIEW_W, VIEW_H);
    for r in &rows {
        assert!(
            r.intersect(&viewport).is_some(),
            "row at y={} is off screen yet was recorded",
            r.origin.y
        );
    }

    // Scrolling to the middle keeps the count bounded and moves the window:
    // the rows now drawn are a different set, not the first ones again.
    scroll(&mut tree, 2000.0);
    let ctx = paint(&tree);
    let scrolled = drawn(&ctx);
    assert!(
        !scrolled.is_empty() && scrolled.len() <= 10,
        "scrolled mid-document, {} rows were recorded",
        scrolled.len()
    );
    let first_top = rows.iter().map(|r| r.origin.y).fold(f32::MAX, f32::min);
    let scrolled_top = scrolled.iter().map(|r| r.origin.y).fold(f32::MAX, f32::min);
    assert!(
        (scrolled_top - first_top).abs() < 60.0,
        "the visible window should stay pinned to the viewport, not follow content"
    );
}

#[test]
fn preview_glyph_count_is_independent_of_document_length() {
    // The motivating case: a markdown preview is a column of `TextWidget`
    // blocks in a `ScrollView`. Before culling, every block emitted its glyphs
    // and they all landed in the shared atlas. Now a 40-block and a 1000-block
    // document cost the same first frame.
    fn glyphs_for(blocks: usize) -> usize {
        let mut tree = WidgetTree::new();
        let root = scroll_view(&mut tree);
        let content = tree.add_child(root, Container::column().gap(8.0));
        for i in 0..blocks {
            tree.add_child(
                content,
                TextWidget::new(format!("paragraph number {i} of the preview body")),
            );
        }
        let mut engine = TextEngine::new();
        let theme = Theme::default();
        tree.compute_layout_with_measure(VIEW_W, VIEW_H, &mut engine, &theme);
        paint(&tree).glyphs.len()
    }

    let short = glyphs_for(40);
    let long = glyphs_for(1000);
    assert!(short > 0, "visible paragraphs must still draw");
    assert_eq!(
        short, long,
        "a 1000-block preview must cost the same screenful as a 40-block one"
    );
}

#[test]
fn offscreen_widgets_skip_paint_entirely() {
    // Culling the draw commands is not enough: the expensive part of a text
    // widget's paint is the shaping that happens *before* the first
    // `draw_glyph`. The tree must not call `paint` at all for a widget parked
    // well outside the clip.
    let mut tree = WidgetTree::new();
    let root = scroll_view(&mut tree);
    let content = tree.add_child(root, Container::column());

    let (visible, visible_count) = PaintCounter::new(100.0);
    tree.add_child(content, visible);
    // Spacer pushing the next counter far below the viewport.
    tree.add_child(content, Container::column().width(VIEW_W).height(2000.0));
    let (offscreen, offscreen_count) = PaintCounter::new(100.0);
    tree.add_child(content, offscreen);

    tree.compute_layout(VIEW_W, VIEW_H);
    paint(&tree);

    assert_eq!(visible_count.get(), 1, "the visible widget must paint");
    assert_eq!(
        offscreen_count.get(),
        0,
        "a widget 2000px below the viewport must not be asked to paint"
    );

    // Scrolling it into view brings it back — culling is per frame, not sticky.
    scroll(&mut tree, 2000.0);
    paint(&tree);
    assert_eq!(
        offscreen_count.get(),
        1,
        "scrolled into view, the widget must paint again"
    );
}

#[test]
fn a_widget_just_outside_the_clip_still_paints() {
    // The skip has slack, because a widget may legitimately draw outside its
    // own layout rect (a focus ring, a shadow halo). A row sitting a few pixels
    // past the viewport edge keeps painting; only the renderer's scissor
    // decides what of it shows.
    let mut tree = WidgetTree::new();
    let root = scroll_view(&mut tree);
    let content = tree.add_child(root, Container::column());
    tree.add_child(content, Container::column().width(VIEW_W).height(VIEW_H));
    let (just_below, count) = PaintCounter::new(100.0);
    tree.add_child(content, just_below);

    tree.compute_layout(VIEW_W, VIEW_H);
    paint(&tree);

    assert_eq!(
        count.get(),
        1,
        "a widget flush against the viewport edge is within the cull margin"
    );
}

#[test]
fn the_focused_widget_is_never_culled() {
    // A focused `Input` republishes the IME cursor area and schedules the next
    // caret blink from its paint. Scrolling it out of view must not silently
    // stop that, so focus opts a node out of the skip.
    let mut tree = WidgetTree::new();
    let root = scroll_view(&mut tree);
    let content = tree.add_child(root, Container::column());
    tree.add_child(content, Container::column().width(VIEW_W).height(2000.0));
    let input = tree.add_child(content, Input::new());

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(VIEW_W, VIEW_H, &mut engine, &theme);
    let mut ectx = EventContext::new();
    tree.focus(Some(input), &mut ectx);

    // Far below the viewport, but focused: its paint still runs. Observed
    // through the clip-space border rect it emits — the *draw* is culled by
    // `PaintContext` (correctly, it is off screen), so what this pins is that
    // the tree reached the widget at all.
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
    assert_eq!(
        tree.focused(),
        Some(input),
        "focus is the precondition for this test"
    );
    assert!(
        ctx.ime_cursor_area().is_some(),
        "a focused input must keep publishing its IME anchor even off screen"
    );
}
