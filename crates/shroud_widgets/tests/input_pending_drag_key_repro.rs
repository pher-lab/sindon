//! Repro: a pointer-select gesture whose deferred hit-test hasn't been resolved
//! by a paint yet, followed by Enter in the *same* event batch, leaves a stuck
//! selection after the newline insert.
//!
//! winit coalesces redraws: each window event only `request_redraw()`s, and the
//! real paint (where `pending_hit` resolves) lands once, after the batch. So a
//! quick drag-select immediately followed by Enter delivers MouseDown +
//! MouseMove + KeyDown(Enter) with *no* intervening paint. Enter sees no
//! selection (the anchor is still unresolved) and just inserts `\n`; the later
//! paint then resolves the stale drag against the post-edit buffer, pinning the
//! anchor to the moved caret and materializing a selection that sticks.

use shroud_core::{Point, Theme};
use shroud_reactive::Signal;
use shroud_text::TextEngine;
use shroud_widgets::event::{EventContext, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::paint::PaintContext;
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input};

const W: f32 = 400.0;
const H: f32 = 200.0;
const TEXT: &str = "aaaaaaaaaa";

struct Field {
    tree: WidgetTree,
    ev: EventContext,
    sel: Signal<Option<(usize, usize)>>,
    idx: usize,
    left_x: f32,
    mid_x: f32,
    right_x: f32,
    y: f32,
}

fn repaint(tree: &WidgetTree) {
    let mut ctx = PaintContext::new(Theme::default());
    tree.paint(&mut ctx);
}

fn build() -> Field {
    let sel = Signal::new(None);
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(W).height(H));
    let idx = tree.add_child(
        root,
        Input::new()
            .multiline()
            .with_value(TEXT)
            .selection_signal(sel),
    );

    let mut engine = TextEngine::new();
    let theme = Theme::default();
    tree.compute_layout_with_measure(W, H, &mut engine, &theme);
    let rect = tree.layout_rect(idx);

    Field {
        tree,
        ev: EventContext::new(),
        sel,
        idx,
        left_x: rect.origin.x + 2.0,
        mid_x: rect.origin.x + 20.0,
        right_x: rect.origin.x + rect.size.width - 5.0,
        y: rect.origin.y + 8.0,
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
    }
    fn move_to(&mut self, x: f32) {
        self.tree.dispatch_event(
            &WidgetEvent::MouseMove {
                position: Point::new(x, self.y),
            },
            &mut self.ev,
        );
    }
    fn up(&mut self, x: f32) {
        self.tree.dispatch_event(
            &WidgetEvent::MouseUp {
                position: Point::new(x, self.y),
                button: MouseButton::Left,
            },
            &mut self.ev,
        );
    }
    fn enter(&mut self) {
        self.ev.modifiers = Default::default();
        self.tree.dispatch_event(
            &WidgetEvent::KeyDown {
                key: Key::Named(NamedKey::Enter),
            },
            &mut self.ev,
        );
    }
    fn key(&mut self, named: NamedKey, mods: Modifiers) {
        self.ev.modifiers = mods;
        self.tree.dispatch_event(
            &WidgetEvent::KeyDown {
                key: Key::Named(named),
            },
            &mut self.ev,
        );
        self.ev.modifiers = Modifiers::NONE;
    }
    fn value(&self) -> String {
        self.tree
            .widget_as::<Input>(self.idx)
            .unwrap()
            .value_clone()
    }
    fn selection(&self) -> Option<(usize, usize)> {
        self.sel.get_clone()
    }
}

#[test]
fn shift_arrow_round_trip_then_plain_arrow_leaves_no_stuck_selection() {
    // The same collapsed-anchor landmine on the keyboard side: Shift+Right then
    // Shift+Left returns the caret to the anchor, leaving `anchor == caret` (an
    // invisible, collapsed selection). A following *plain* ArrowLeft steps the
    // caret off the anchor and — if the anchor isn't dropped — the selection
    // re-expands and sticks.
    let mut f = build();

    f.down(f.right_x); // focus, caret at end (10)
    f.up(f.right_x);
    repaint(&f.tree);

    let shift = {
        let mut m = Modifiers::NONE;
        m.shift = true;
        m
    };
    f.key(NamedKey::ArrowLeft, shift); // select one char left (9..10)
    f.key(NamedKey::ArrowRight, shift); // back to 10 -> collapsed anchor at 10
    assert_eq!(
        f.selection(),
        None,
        "a shift round-trip back to the anchor shows no selection"
    );

    f.key(NamedKey::ArrowLeft, Modifiers::NONE); // plain move off the anchor

    assert_eq!(
        f.selection(),
        None,
        "a plain caret move must not resurrect the collapsed selection"
    );
}

#[test]
fn tiny_drag_then_enter_leaves_no_stuck_selection() {
    // The gesture *is* resolved by a paint (not batched), but a "そぶり" drag
    // that never leaves the pressed character resolves to a zero-length
    // (collapsed) selection: the anchor is planted, equal to the caret, so it's
    // invisible. `delete_selection` only clears the anchor for a *non*-collapsed
    // selection, so Enter's newline insert moves the caret out from under the
    // lingering anchor and the "selection" re-expands and sticks.
    let mut f = build();

    // Focus + caret at end.
    f.down(f.right_x);
    f.up(f.right_x);
    repaint(&f.tree);

    // Press and micro-wiggle within the same spot, resolving each event with a
    // paint (so `pending_hit` is empty by the time Enter arrives — the batched
    // guard can't catch this one).
    f.down(f.mid_x);
    repaint(&f.tree);
    f.move_to(f.mid_x);
    repaint(&f.tree);
    f.up(f.mid_x);
    repaint(&f.tree);
    assert_eq!(
        f.selection(),
        None,
        "a zero-length drag shows no selection before Enter"
    );

    f.enter();
    repaint(&f.tree);

    assert_eq!(
        f.selection(),
        None,
        "Enter's newline must not resurrect the collapsed selection"
    );
}

#[test]
fn drag_gesture_batched_with_enter_leaves_no_stuck_selection() {
    let mut f = build();

    // Focus + park the caret at the end (aaaaaaaaaa|).
    f.down(f.right_x);
    f.up(f.right_x);
    repaint(&f.tree);
    assert_eq!(
        f.selection(),
        None,
        "sanity: no selection after the focus click"
    );

    // A quick select gesture over the text, then Enter — all in ONE batch,
    // no paint in between (models winit coalescing the redraws).
    f.down(f.left_x);
    f.move_to(f.mid_x);
    f.enter();

    // The deferred paint finally runs.
    repaint(&f.tree);

    assert_eq!(
        f.value(),
        "aaaaaaaaaa\n",
        "Enter should insert a newline at the caret"
    );
    assert_eq!(
        f.selection(),
        None,
        "the newline insert must not leave a stuck selection behind"
    );
}
