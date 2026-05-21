//! Editor pane — title input + multiline body, plus the Lock button.
//!
//! Both inputs are bound to `Signal<String>` so that switching notes
//! (via the sidebar) only needs `signal.set(...)` — the Input widget
//! rebases its buffer from the signal on the next paint without us having
//! to rebuild the subtree. `on_change` writes back to the selected note
//! in `AppState`.

use std::cell::RefCell;
use std::rc::Rc;

use shroud::core::Color;
use shroud::reactive::Signal;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, Input, TextWidget};

use crate::lock_screen;
use crate::state::{AppState, Phase};

pub fn build(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    title_sig: Signal<String>,
    body_sig: Signal<String>,
) {
    let pane = tree.add_child(
        parent,
        Container::column()
            .grow(1.0)
            .height_full()
            .padding(24.0)
            .gap(12.0)
            .background(Color::rgb(0.06, 0.06, 0.09)),
    );

    // Header: status text on the left, Lock button on the right. Status
    // reads selection from state so it nudges the user when nothing is
    // selected ("No note selected — click + New").
    let header = tree.add_child(pane, Container::row().gap(12.0).align_center());

    let status_state = Rc::clone(&state);
    tree.add_child(
        header,
        TextWidget::reactive(move || match &status_state.borrow().phase {
            Phase::Unlocked { notes, selected, .. } => {
                if let Some(sel) = selected {
                    if let Some(note) = notes.iter().find(|n| n.id == *sel) {
                        let title = if note.title.is_empty() {
                            "(untitled)"
                        } else {
                            note.title.as_str()
                        };
                        format!("Editing: {}", title)
                    } else {
                        String::from("No note selected.")
                    }
                } else {
                    String::from("No note selected \u{2014} click + New to start.")
                }
            }
            _ => String::new(),
        })
        .color(Color::rgb(0.6, 0.6, 0.7)),
    );

    // Spacer pushes the Lock button to the far right.
    tree.add_child(header, Container::row().grow(1.0));

    let lock_state = Rc::clone(&state);
    tree.add_child(
        header,
        Button::new("Lock").radius(8.0).on_click(move |ctx| {
            // Re-encrypt the current notes into the vault, then transition
            // to the lock screen. `lock_and_seal` drops the key + plaintext
            // notes; Zeroizing ensures the key is wiped.
            lock_state.borrow_mut().lock_and_seal();
            let next = Rc::clone(&lock_state);
            ctx.replace_screen(move |tree| lock_screen::build(tree, next));
        }),
    );

    // Title input (single-line, full width).
    let title_state = Rc::clone(&state);
    tree.add_child(
        pane,
        Input::new()
            .placeholder("Title")
            .value(title_sig)
            .font_size(20.0)
            .on_change(move |new_title, _ctx| {
                write_selected(&title_state, |note| {
                    note.title = new_title.to_string();
                });
            }),
    );

    // Body input (multiline, grows to fill remaining height).
    //
    // Pass `grow(1.0)` via a wrapper container — Input itself doesn't take
    // flex-grow on its own style, so wrap it.
    let body_wrap = tree.add_child(
        pane,
        Container::column().width_full().grow(1.0).padding(0.0),
    );

    let body_state = Rc::clone(&state);
    tree.add_child(
        body_wrap,
        Input::new()
            .placeholder("Start writing…")
            .multiline()
            .lines(16)
            .value(body_sig)
            .on_change(move |new_body, _ctx| {
                write_selected(&body_state, |note| {
                    note.body = new_body.to_string();
                });
            }),
    );
}

/// Apply `f` to the currently selected note's mutable state and mark
/// it dirty so the auto-save tick (`AppState::flush_dirty`) writes the
/// change back to SQLCipher within a tick interval. No-op when no
/// note is selected or the app is not in `Unlocked`.
fn write_selected<F>(state: &Rc<RefCell<AppState>>, f: F)
where
    F: FnOnce(&mut crate::state::Note),
{
    let mut s = state.borrow_mut();
    let Phase::Unlocked {
        notes, selected, ..
    } = &mut s.phase
    else {
        return;
    };
    let Some(sel) = *selected else { return };
    if let Some(note) = notes.iter_mut().find(|n| n.id == sel) {
        f(note);
    }
    // Drop the borrow before mark_selected_dirty takes another mut
    // borrow. (RefCell will panic on overlapping borrows.)
    drop(s);
    state.borrow_mut().mark_selected_dirty();
}
