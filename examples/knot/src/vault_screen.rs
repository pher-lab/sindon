//! Vault screen — the post-unlock app shell.
//!
//! Layout is a single row at the root: sidebar pane (fixed width) on the
//! left, editor pane (flex-grow) on the right. The Lock button lives in
//! the editor's header so it's adjacent to the content the user just
//! decrypted, matching the Knot v0.7.0 layout.
//!
//! The two `Signal<String>` (title + body) live for the lifetime of this
//! screen tree; switching notes calls `signal.set(...)` to rebase the
//! editor's bound `Input`s on the next paint.

use std::cell::RefCell;
use std::rc::Rc;

use shroud::reactive::Signal;
use shroud::widgets::Container;
use shroud::widgets::tree::WidgetTree;

use crate::editor;
use crate::sidebar;
use crate::state::{AppState, Phase};

pub fn build(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    // Seed the editor signals from the initially-selected note (which
    // `lock_screen::try_unlock` set to the first note if any exist).
    let (initial_title, initial_body) = match &state.borrow().phase {
        Phase::Unlocked {
            notes, selected, ..
        } => selected
            .and_then(|sel| notes.iter().find(|n| n.id == sel))
            .map(|n| (n.title.clone(), n.body.clone()))
            .unwrap_or_default(),
        _ => (String::new(), String::new()),
    };

    let title_sig = Signal::new(initial_title);
    let body_sig = Signal::new(initial_body);
    // Edit ⇄ preview toggle for the editor pane. Lives here (alongside the
    // title/body signals) because the sidebar resets it to edit mode whenever
    // the active note changes — selecting/creating/deleting a note drops you
    // back into the editor rather than leaving a stale preview of the old
    // body on screen.
    let preview_sig = Signal::new(false);

    let root = tree.set_root(Container::row().width_full().height_full());

    sidebar::build(
        tree,
        root,
        Rc::clone(&state),
        title_sig,
        body_sig,
        preview_sig,
    );

    editor::build(tree, root, state, title_sig, body_sig, preview_sig);
}
