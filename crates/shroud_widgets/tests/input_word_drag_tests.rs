//! Word / line drag-select: end-to-end widget proof.
//!
//! Reported by dogfooding: typing "ノートNote", double-clicking "Note", then —
//! without releasing — dragging into "ノート" used to *drop* "Note" from the
//! selection and leave only (part of) "ノート" selected. The drag had silently
//! degraded to a character-level caret drag anchored at the double-clicked
//! word's left edge, instead of snapping by whole words with the first word
//! pinned as the anchor.
//!
//! Unlike the pure `union_drag_units` unit tests in `input.rs`, these drive the
//! real event -> paint -> resolve pipeline (`pending_select` held across the
//! whole gesture, `drag_origin` re-deriving the anchor unit) so the wiring —
//! not just the arithmetic — is pinned.

use shroud_core::{Point, Theme};
use shroud_reactive::Signal;
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, MouseButton, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input};

// Wide enough that "ノートNote" never wraps, so byte offsets map to a stable
// single visual row.
const W: f32 = 400.0;
const H: f32 = 120.0;

// "ノート" is a 9-byte katakana run [0, 9); "Note" is [9, 13).
const TEXT: &str = "ノートNote";

struct Field {
    tree: WidgetTree,
    ev: EventContext,
    sel: Signal<Option<(usize, usize)>>,
    left_x: f32,  // a point over "ノート" (line start)
    right_x: f32, // a point over "Note" (past the line end -> word_bounds looks left)
    y: f32,
}

// Repaint after every event, like the real app — the deferred click / drag
// hit-test resolves in `paint`, so the selection only updates here.
fn repaint(tree: &WidgetTree) {
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
}

fn build() -> Field {
    let sel = Signal::new(None);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    let idx = tree.add_child(root, Input::new().with_value(TEXT).selection_signal(sel));

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);
    let rect = tree.layout_rect(idx);

    Field {
        tree,
        ev: EventContext::new(),
        sel,
        // Just inside the left edge: hit-tests to offset 0 -> word "ノート".
        left_x: rect.origin.x + 2.0,
        // Well past the (short) text: hit-tests to the line end -> word "Note".
        right_x: rect.origin.x + rect.size.width - 5.0,
        y: rect.origin.y + rect.size.height / 2.0,
    }
}

impl Field {
    fn down(&mut self, x: f32) {
        self.tree.dispatch_event(
            &WidgetEvent::MouseDown {
                position: Point::new(x, self.y),
                button: MouseButton::Left,
            },
            &mut self.ev,
        );
        repaint(&self.tree);
    }

    fn up(&mut self, x: f32) {
        self.tree.dispatch_event(
            &WidgetEvent::MouseUp {
                position: Point::new(x, self.y),
                button: MouseButton::Left,
            },
            &mut self.ev,
        );
        repaint(&self.tree);
    }

    fn move_to(&mut self, x: f32) {
        self.tree.dispatch_event(
            &WidgetEvent::MouseMove {
                position: Point::new(x, self.y),
            },
            &mut self.ev,
        );
        repaint(&self.tree);
    }

    fn selection(&self) -> Option<(usize, usize)> {
        self.sel.get_clone()
    }
}

#[test]
fn double_click_then_drag_left_keeps_first_word_selected() {
    let mut f = build();

    // Double-click "Note": down, up, down-and-hold (no release).
    f.down(f.right_x);
    f.up(f.right_x);
    f.down(f.right_x);

    assert_eq!(
        f.selection(),
        Some((9, 13)),
        "the double-click should select just the latin word \"Note\""
    );

    // Drag left into "ノート" without releasing.
    f.move_to(f.left_x);

    assert_eq!(
        f.selection(),
        Some((0, 13)),
        "word-snap drag must keep \"Note\" selected and extend over \"ノート\" — \
         the pre-fix bug left only Some((0, 9))"
    );
}

#[test]
fn double_click_then_drag_right_extends_by_whole_words() {
    let mut f = build();

    // Double-click "ノート" (left), then drag right into "Note".
    f.down(f.left_x);
    f.up(f.left_x);
    f.down(f.left_x);

    assert_eq!(
        f.selection(),
        Some((0, 9)),
        "the double-click should select the katakana word \"ノート\""
    );

    f.move_to(f.right_x);

    assert_eq!(
        f.selection(),
        Some((0, 13)),
        "dragging right must snap to the whole \"Note\" word, not split it"
    );
}

#[test]
fn releasing_ends_word_snap_so_a_later_plain_drag_is_character_level() {
    let mut f = build();

    // Word-drag select everything, then release.
    f.down(f.right_x);
    f.up(f.right_x);
    f.down(f.right_x);
    f.move_to(f.left_x);
    f.up(f.left_x);
    assert_eq!(f.selection(), Some((0, 13)));

    // A fresh single press collapses the caret (no stale word/line snap unit).
    f.down(f.left_x);
    assert_eq!(
        f.selection(),
        None,
        "a plain single click after a word-drag must clear the selection, \
         proving the held snap unit was reset on release"
    );
}
