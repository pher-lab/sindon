//! Integration tests for the C-3 clipboard image-paste primitive.
//!
//! Covers `WidgetTree::on_image_paste` registration, `dispatch_image_paste`
//! invoking the handler with the pasted PNG bytes, the no-handler no-op, and
//! the screen-scoped lifetime contract (a `replace_screen` transition clears
//! the handler so it never fires on the next screen). Mirrors
//! `file_drop_tests.rs` — the two window-level hooks share a design.

use std::cell::RefCell;
use std::rc::Rc;

use shroud_widgets::Container;
use shroud_widgets::event::EventContext;
use shroud_widgets::tree::WidgetTree;

#[test]
fn dispatch_image_paste_invokes_handler_with_bytes() {
    let seen: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));

    let mut tree = WidgetTree::new();
    tree.set_root(Container::column());
    let captured = Rc::clone(&seen);
    tree.on_image_paste(move |png, _ctx| captured.borrow_mut().push(png.to_vec()));

    let mut ctx = EventContext::new();
    tree.dispatch_image_paste(&[1, 2, 3], &mut ctx);
    // A second paste — the handler must fire again and stay registered.
    tree.dispatch_image_paste(&[4, 5, 6], &mut ctx);

    assert_eq!(
        *seen.borrow(),
        vec![vec![1, 2, 3], vec![4, 5, 6]],
        "handler receives each pasted blob and stays registered across pastes"
    );
}

#[test]
fn dispatch_image_paste_without_handler_is_noop() {
    // No handler registered (e.g. a screen that doesn't accept pasted
    // images): dispatch must not panic and simply does nothing.
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column());
    let mut ctx = EventContext::new();
    tree.dispatch_image_paste(&[0xff], &mut ctx);
}

#[test]
fn replace_screen_clears_image_paste_handler() {
    // The handler is screen-scoped: after a `replace_screen` transition it
    // must not fire, since it typically captures the torn-down screen's
    // signals. The new screen re-registers if it wants pasted images.
    let fired = Rc::new(RefCell::new(0u32));

    let mut tree = WidgetTree::new();
    tree.set_root(Container::column());
    let captured = Rc::clone(&fired);
    tree.on_image_paste(move |_png, _ctx| *captured.borrow_mut() += 1);

    // Sanity: it fires before the swap.
    let mut ctx = EventContext::new();
    tree.dispatch_image_paste(&[1], &mut ctx);
    assert_eq!(
        *fired.borrow(),
        1,
        "handler fires on the screen that set it"
    );

    // Transition to a new screen that registers nothing.
    ctx.replace_screen(|t| {
        t.set_root(Container::row());
    });
    tree.apply_pending_commands(&mut ctx);

    // The paste now lands on a screen with no handler — the stale one is gone.
    tree.dispatch_image_paste(&[2], &mut ctx);
    assert_eq!(
        *fired.borrow(),
        1,
        "replace_screen clears the image-paste handler — no fire on the new screen"
    );
}
