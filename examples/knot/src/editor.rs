//! Editor pane — title input + multiline body, plus the Lock button.
//!
//! Both inputs are bound to `Signal<String>` so that switching notes
//! (via the sidebar) only needs `signal.set(...)` — the Input widget
//! rebases its buffer from the signal on the next paint without us having
//! to rebuild the subtree. `on_change` writes back to the selected note
//! in `AppState`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shroud::reactive::{Reactive, Signal};
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, Input, ScrollView, TextWidget};

use crate::lock_screen;
use crate::preview;
use crate::settings;
use crate::state::{AppState, Phase};

/// True when the app is unlocked *and* a note is selected — the condition for
/// showing any editing surface at all (inputs or preview). Shared by the
/// edit/preview area visibilities and the Preview toggle button so they flip
/// together.
fn note_selected(state: &Rc<RefCell<AppState>>) -> bool {
    matches!(
        &state.borrow().phase,
        Phase::Unlocked {
            selected: Some(_),
            ..
        }
    )
}

pub fn build(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    title_sig: Signal<String>,
    body_sig: Signal<String>,
    preview_sig: Signal<bool>,
) {
    let pane = tree.add_child(
        parent,
        Container::column()
            // `flex: 1 1 0`, not just `grow`: without a zero basis the pane's
            // flex-basis is `auto` = its max-content width, which for a large
            // preview heading (especially space-less CJK) is the whole
            // unwrapped line. That overflows the root row and shrinks the
            // fixed-width sidebar instead of letting the heading wrap. A zero
            // basis pins the pane to the row's leftover width so the content
            // wraps within it and the sidebar keeps its width.
            .flex_basis(0.0)
            .grow(1.0)
            .height_full()
            .padding(24.0)
            .gap(12.0)
            .background(settings::background()),
    );

    // Header: status text on the left, then a spacer, then the Preview/Edit
    // toggle and the Lock button on the right. Status reads selection from
    // state so it nudges the user when nothing is selected ("No note selected
    // — click + New").
    let header = tree.add_child(pane, Container::row().gap(12.0).align_center());

    let status_state = Rc::clone(&state);
    tree.add_child(
        header,
        TextWidget::reactive(move || match &status_state.borrow().phase {
            Phase::Unlocked {
                notes, selected, ..
            } => {
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
        .color(settings::on_surface_variant()),
    );

    // Spacer pushes the trailing buttons to the far right.
    tree.add_child(header, Container::row().grow(1.0));

    // Preview / Edit toggle. The content column it rebuilds is created below
    // (inside `preview_area`); its index is stashed in this cell because this
    // closure captures the cell at build time and only learns the real index
    // once that column is inserted. Hidden when no note is selected so the
    // header's "No note selected" prompt stands alone.
    let preview_content_cell: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let toggle_cell = Rc::clone(&preview_content_cell);
    let toggle_visible_state = Rc::clone(&state);
    // Navigation handle for `[[wikilink]]` clicks inside the preview: clicking
    // one selects the matching note and drops back to the editor (see
    // `preview::WikiNav`). Cloned per render so each rebuilt preview subtree
    // carries its own.
    let nav = preview::WikiNav::new(Rc::clone(&state), title_sig, body_sig, preview_sig);
    tree.add_child(
        header,
        Button::reactive_label(move || {
            if preview_sig.get() {
                "Edit".to_string()
            } else {
                "Preview".to_string()
            }
        })
        .radius(8.0)
        .visible(Reactive::derive(move || {
            note_selected(&toggle_visible_state)
        }))
        .on_click(move |ctx| {
            if preview_sig.get() {
                // Preview → edit: just flip back; the inputs hold the live body.
                preview_sig.set(false);
            } else {
                // Edit → preview: re-render the current body into the preview
                // column, *then* show it. Rebuilding on every entry keeps the
                // preview in sync with edits made since it was last shown
                // (there is no reactive markdown widget — see `preview.rs`).
                let body = body_sig.get_clone();
                let parent_idx = toggle_cell.get();
                let nav = nav.clone();
                ctx.rebuild_children(parent_idx, move |tree, parent| {
                    preview::render(tree, parent, &body, Some(&nav));
                });
                preview_sig.set(true);
            }
        }),
    );

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

    // Editor area: title + body inputs. Hidden via `display: none` when no
    // note is selected (so the header's "No note selected" prompt stands alone
    // instead of showing inputs that look editable but silently drop typing —
    // `write_selected` no-ops without a selection) *and* while previewing, so
    // the rendered preview replaces the raw-markdown inputs in place.
    let area_state = Rc::clone(&state);
    let editor_area = tree.add_child(
        pane,
        Container::column()
            .width_full()
            .grow(1.0)
            .gap(12.0)
            .visible(Reactive::derive(move || {
                note_selected(&area_state) && !preview_sig.get()
            })),
    );

    // Title input (single-line, full width).
    let title_state = Rc::clone(&state);
    tree.add_child(
        editor_area,
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
        editor_area,
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

    // Preview area: a scrollable rendered-markdown view, shown in place of the
    // inputs while previewing. Mirrors `editor_area`'s visibility, inverted on
    // the preview flag. Its content column starts empty and is (re)populated by
    // the header toggle's `rebuild_children` each time we enter preview.
    let preview_state = Rc::clone(&state);
    let preview_area = tree.add_child(
        pane,
        Container::column()
            .width_full()
            .grow(1.0)
            .visible(Reactive::derive(move || {
                note_selected(&preview_state) && preview_sig.get()
            })),
    );
    let preview_scroll = tree.add_child(preview_area, ScrollView::new().width_full().grow(1.0));
    let preview_content =
        tree.add_child(preview_scroll, Container::column().width_full().gap(12.0));
    preview_content_cell.set(preview_content);
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
