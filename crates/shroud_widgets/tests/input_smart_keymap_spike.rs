//! B-1 ③ smart-keymap spike: widget-level proof of the `on_enter` /
//! `on_backspace` hooks.
//!
//! The framework owns the *mechanism* — apply an app-supplied [`KeyEdit`] as one
//! discrete undo step, validate it, and otherwise fall through to the key's
//! default behavior — while the app owns the *policy* (what a list marker is).
//! These tests pin the mechanism with a toy bullet handler standing in for the
//! markdown logic (which is tested in Knot's own `smart_keymap` module):
//!
//! 1. Enter continues / exits a list item via the returned edit,
//! 2. a `None` (or malformed) edit falls through to the default newline / delete,
//! 3. the hooks are gated correctly (multi-line only, no selection, caret > 0),
//! 4. a structural edit is a single undo step.

use shroud_core::Point;
use shroud_widgets::event::{EventContext, Key, Modifiers, MouseButton, NamedKey, WidgetEvent};
use shroud_widgets::tree::WidgetTree;
use shroud_widgets::{Container, Input, KeyEdit};
use std::cell::Cell;
use std::rc::Rc;

/// Build a focused field seeded with `with_value` (caret at end). Focus comes
/// from the tree's click-to-focus on MouseDown; the precise caret hit-test is a
/// paint-time job we deliberately skip, so the caret stays where `with_value`
/// left it — at the end of the buffer.
fn focused(input: Input) -> (WidgetTree, usize, EventContext) {
    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(200.0));
    let idx = tree.add_child(root, input);
    tree.compute_layout(400.0, 200.0);
    let mut ctx = EventContext::new();
    let rect = tree.layout_rect(idx);
    tree.dispatch_event(
        &WidgetEvent::MouseDown {
            position: Point::new(rect.origin.x + 5.0, rect.origin.y + 5.0),
            button: MouseButton::Left,
        },
        &mut ctx,
    );
    (tree, idx, ctx)
}

/// Dispatch a KeyDown with `mods` held, then reset modifiers so a following
/// event isn't read as a chord.
fn key(tree: &mut WidgetTree, k: Key, mods: Modifiers, ctx: &mut EventContext) {
    ctx.modifiers = mods;
    tree.dispatch_event(&WidgetEvent::KeyDown { key: k }, ctx);
    ctx.modifiers = Modifiers::NONE;
}

fn enter(tree: &mut WidgetTree, ctx: &mut EventContext) {
    key(tree, Key::Named(NamedKey::Enter), Modifiers::NONE, ctx);
}

fn backspace(tree: &mut WidgetTree, ctx: &mut EventContext) {
    key(tree, Key::Named(NamedKey::Backspace), Modifiers::NONE, ctx);
}

fn value(tree: &WidgetTree, idx: usize) -> String {
    tree.widget_as::<Input>(idx).unwrap().value_clone()
}

fn cursor(tree: &WidgetTree, idx: usize) -> usize {
    tree.widget_as::<Input>(idx).unwrap().cursor()
}

/// Byte range `[start, end)` of the hard line containing `cursor`.
fn line_bounds(text: &str, cursor: usize) -> (usize, usize) {
    let start = text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[cursor..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(text.len());
    (start, end)
}

/// Toy markdown handler: continue a `- ` bullet on Enter, or — when the bullet
/// is empty — remove the marker (exit the list). Stand-in for Knot's real one.
fn bullet_enter(text: &str, cursor: usize) -> Option<KeyEdit> {
    let (line_start, line_end) = line_bounds(text, cursor);
    let rest = text[line_start..line_end].strip_prefix("- ")?;
    if rest.trim().is_empty() {
        Some(KeyEdit {
            replace: line_start..line_end,
            insert: String::new(),
            caret: line_start,
        })
    } else {
        Some(KeyEdit {
            replace: cursor..cursor,
            insert: "\n- ".to_string(),
            caret: cursor + 3,
        })
    }
}

/// Toy backspace handler: delete a whole `- ` marker when the caret sits right
/// after it (nothing else before it on the line).
fn bullet_backspace(text: &str, cursor: usize) -> Option<KeyEdit> {
    let (line_start, _) = line_bounds(text, cursor);
    if &text[line_start..cursor] == "- " {
        Some(KeyEdit {
            replace: line_start..cursor,
            insert: String::new(),
            caret: line_start,
        })
    } else {
        None
    }
}

#[test]
fn enter_continues_a_list_item() {
    let (mut tree, idx, mut ctx) = focused(
        Input::new()
            .multiline()
            .with_value("- a")
            .on_enter(bullet_enter),
    );
    enter(&mut tree, &mut ctx);
    assert_eq!(value(&tree, idx), "- a\n- ");
    assert_eq!(cursor(&tree, idx), 6, "caret lands after the new marker");
}

#[test]
fn enter_on_empty_item_removes_the_marker() {
    // The bullet has no content, so Enter exits the list: the marker line is
    // replaced with nothing and the caret returns to the line start.
    let (mut tree, idx, mut ctx) = focused(
        Input::new()
            .multiline()
            .with_value("- ")
            .on_enter(bullet_enter),
    );
    enter(&mut tree, &mut ctx);
    assert_eq!(value(&tree, idx), "");
    assert_eq!(cursor(&tree, idx), 0);
}

#[test]
fn enter_falls_through_when_handler_returns_none() {
    // A non-list line: the hook returns None, so Enter inserts a plain newline.
    let (mut tree, idx, mut ctx) = focused(
        Input::new()
            .multiline()
            .with_value("plain")
            .on_enter(bullet_enter),
    );
    enter(&mut tree, &mut ctx);
    assert_eq!(value(&tree, idx), "plain\n");
    assert_eq!(cursor(&tree, idx), 6);
}

#[test]
fn enter_hook_is_skipped_while_text_is_selected() {
    // A selection always falls through to the default newline — the hook never
    // sees a selection. (Default multi-line Enter inserts at the caret without
    // clearing the selection; the point here is only that no marker was added.)
    let (mut tree, idx, mut ctx) = focused(
        Input::new()
            .multiline()
            .with_value("- a")
            .on_enter(bullet_enter),
    );
    key(&mut tree, Key::Character('a'), Modifiers::CTRL, &mut ctx); // select all
    enter(&mut tree, &mut ctx);
    assert!(
        !value(&tree, idx).contains("\n- "),
        "the list-continuation marker must not be inserted while selecting"
    );
}

#[test]
fn malformed_edit_falls_through_without_panicking() {
    // An out-of-bounds range and a range that splits a multi-byte char must
    // both be rejected, degrading to the default newline rather than crashing.
    let oob = Input::new().multiline().with_value("x").on_enter(|_t, c| {
        Some(KeyEdit {
            replace: 0..9999,
            insert: "BAD".to_string(),
            caret: c,
        })
    });
    let (mut tree, idx, mut ctx) = focused(oob);
    enter(&mut tree, &mut ctx);
    assert_eq!(value(&tree, idx), "x\n", "out-of-bounds edit is ignored");

    let split = Input::new()
        .multiline()
        .with_value("あ") // 3 UTF-8 bytes
        .on_enter(|_t, _c| {
            Some(KeyEdit {
                replace: 0..1, // mid-codepoint
                insert: String::new(),
                caret: 0,
            })
        });
    let (mut tree, idx, mut ctx) = focused(split);
    enter(&mut tree, &mut ctx);
    assert_eq!(value(&tree, idx), "あ\n", "char-splitting edit is ignored");
}

#[test]
fn backspace_deletes_a_whole_marker() {
    let (mut tree, idx, mut ctx) = focused(
        Input::new()
            .multiline()
            .with_value("- ")
            .on_backspace(bullet_backspace),
    );
    backspace(&mut tree, &mut ctx);
    assert_eq!(value(&tree, idx), "");
    assert_eq!(cursor(&tree, idx), 0);
}

#[test]
fn backspace_falls_through_to_single_char_when_handler_returns_none() {
    let (mut tree, idx, mut ctx) = focused(
        Input::new()
            .multiline()
            .with_value("abc")
            .on_backspace(bullet_backspace),
    );
    backspace(&mut tree, &mut ctx);
    assert_eq!(
        value(&tree, idx),
        "ab",
        "a non-marker line deletes one char"
    );
    assert_eq!(cursor(&tree, idx), 2);
}

#[test]
fn smart_enter_is_a_single_undo_step() {
    // The structural insert must be one undo step — Ctrl+Z restores the whole
    // pre-Enter buffer, not just part of the inserted marker.
    let (mut tree, idx, mut ctx) = focused(
        Input::new()
            .multiline()
            .with_value("- a")
            .on_enter(bullet_enter),
    );
    enter(&mut tree, &mut ctx);
    assert_eq!(value(&tree, idx), "- a\n- ");
    key(&mut tree, Key::Character('z'), Modifiers::CTRL, &mut ctx);
    assert_eq!(
        value(&tree, idx),
        "- a",
        "one Ctrl+Z reverts the whole edit"
    );
}

#[test]
fn single_line_enter_ignores_the_hook() {
    // The Enter hook is multi-line only; a single-line field never consults it
    // (Enter there is for submit), so the buffer is untouched.
    let (mut tree, idx, mut ctx) = focused(Input::new().with_value("- a").on_enter(bullet_enter));
    enter(&mut tree, &mut ctx);
    assert_eq!(
        value(&tree, idx),
        "- a",
        "single-line Enter must not run the hook"
    );
}

#[test]
fn empty_buffer_backspace_hits_the_empty_hook_not_the_smart_one() {
    // The two backspace hooks never overlap: an empty buffer (caret 0) routes to
    // on_backspace_empty, and the smart hook (which needs caret > 0) is never
    // consulted.
    let empty_fired = Rc::new(Cell::new(false));
    let smart_called = Rc::new(Cell::new(false));
    let ef = Rc::clone(&empty_fired);
    let sc = Rc::clone(&smart_called);
    let input = Input::new()
        .multiline()
        .on_backspace(move |_t, _c| {
            sc.set(true);
            None
        })
        .on_backspace_empty(move |_ctx| ef.set(true));
    let (mut tree, _idx, mut ctx) = focused(input);
    backspace(&mut tree, &mut ctx);
    assert!(
        empty_fired.get(),
        "empty-buffer Backspace fires on_backspace_empty"
    );
    assert!(
        !smart_called.get(),
        "the smart hook must not be consulted on an empty buffer"
    );
}
