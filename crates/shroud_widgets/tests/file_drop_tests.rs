//! Integration tests for the B-6 file-drop primitive.
//!
//! Covers `WidgetTree::on_file_drop` registration, `dispatch_file_drop`
//! invoking the handler with the dropped path, the no-handler no-op, and
//! the screen-scoped lifetime contract (a `replace_screen` transition
//! clears the handler so it never fires on the next screen).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use shroud_widgets::Container;
use shroud_widgets::event::EventContext;
use shroud_widgets::tree::WidgetTree;

#[test]
fn dispatch_file_drop_invokes_handler_with_path() {
    let seen: Rc<RefCell<Vec<PathBuf>>> = Rc::new(RefCell::new(Vec::new()));

    let mut tree = WidgetTree::new();
    tree.set_root(Container::column());
    let captured = Rc::clone(&seen);
    tree.on_file_drop(move |path, _ctx| captured.borrow_mut().push(path.to_path_buf()));

    let mut ctx = EventContext::new();
    tree.dispatch_file_drop(Path::new("/tmp/a.png"), &mut ctx);
    // A second drop — winit delivers a multi-file drop as separate events,
    // so the handler must fire once per call (and stay registered between).
    tree.dispatch_file_drop(Path::new("/tmp/b.jpg"), &mut ctx);

    assert_eq!(
        *seen.borrow(),
        vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.jpg")],
        "handler receives each dropped path and stays registered across drops"
    );
}

#[test]
fn dispatch_file_drop_without_handler_is_noop() {
    // No handler registered (e.g. a screen that doesn't accept drops):
    // dispatch must not panic and simply does nothing.
    let mut tree = WidgetTree::new();
    tree.set_root(Container::column());
    let mut ctx = EventContext::new();
    tree.dispatch_file_drop(Path::new("anything"), &mut ctx);
}

#[test]
fn replace_screen_clears_file_drop_handler() {
    // The handler is screen-scoped: after a `replace_screen` transition it
    // must not fire, since it typically captures the torn-down screen's
    // signals. The new screen re-registers if it wants drops.
    let fired = Rc::new(RefCell::new(0u32));

    let mut tree = WidgetTree::new();
    tree.set_root(Container::column());
    let captured = Rc::clone(&fired);
    tree.on_file_drop(move |_path, _ctx| *captured.borrow_mut() += 1);

    // Sanity: it fires before the swap.
    let mut ctx = EventContext::new();
    tree.dispatch_file_drop(Path::new("before.png"), &mut ctx);
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

    // The drop now lands on a screen with no handler — the stale one is gone.
    tree.dispatch_file_drop(Path::new("after.png"), &mut ctx);
    assert_eq!(
        *fired.borrow(),
        1,
        "replace_screen clears the file-drop handler — no fire on the new screen"
    );
}
