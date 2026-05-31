//! Sidebar — header with "+ New" + scrollable note list.
//!
//! The list is a `ScrollView > Container::column`. Each row is a `Button`
//! whose label and background are reactive — title edits in the editor
//! reflect live, and the selected row stays highlighted across redraws.
//!
//! Mutations that change the row set (add / delete) call
//! [`rebuild_list_into`] via `EventContext::rebuild_children`. The list
//! parent index is stashed in an `Rc<Cell<usize>>` because the closures
//! that trigger rebuilds (the "+ New" button, future delete buttons)
//! capture the cell at build time and only learn the real index after the
//! parent widget is inserted into the tree.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shroud::core::Color;
use shroud::reactive::{Reactive, Signal};
use shroud::widgets::layer::LayerOptions;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, ScrollView, TextWidget};

use crate::settings;
use crate::state::{AppState, Note, NoteId, Phase};

const SIDEBAR_WIDTH: f32 = 260.0;

/// Build the whole sidebar pane and return `(pane_idx, list_idx_cell)`.
///
/// The list cell starts holding the real list-parent index, but is also
/// kept around so that "+ New" / delete handlers know which subtree to
/// rebuild.
pub fn build(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    title_sig: Signal<String>,
    body_sig: Signal<String>,
    preview_sig: Signal<bool>,
) -> Rc<Cell<usize>> {
    let pane = tree.add_child(
        parent,
        Container::column()
            .width(SIDEBAR_WIDTH)
            .height_full()
            .padding(16.0)
            .gap(12.0)
            .background(settings::surface()),
    );

    let header = tree.add_child(pane, Container::row().gap(8.0).align_center());
    tree.add_child(header, TextWidget::new("Knot").font_size(20.0));
    // Spacer pushes the + New button to the right edge.
    tree.add_child(header, Container::row().grow(1.0));
    let new_state = Rc::clone(&state);
    let list_cell: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let new_cell = Rc::clone(&list_cell);
    tree.add_child(
        header,
        Button::new("+ New").radius(6.0).on_click(move |ctx| {
            if create_note(&new_state, &title_sig, &body_sig, preview_sig).is_some() {
                let parent_idx = new_cell.get();
                let s = Rc::clone(&new_state);
                let c = Rc::clone(&new_cell);
                ctx.rebuild_children(parent_idx, move |tree, parent| {
                    rebuild_list_into(tree, parent, s, title_sig, body_sig, preview_sig, c);
                });
            }
        }),
    );

    let scroll = tree.add_child(pane, ScrollView::new().width_full().grow(1.0));
    let list = tree.add_child(scroll, Container::column().width_full().gap(4.0));
    list_cell.set(list);

    rebuild_list_into(
        tree,
        list,
        Rc::clone(&state),
        title_sig,
        body_sig,
        preview_sig,
        Rc::clone(&list_cell),
    );

    // Pinned to the bottom of the pane (the scroll above takes grow:1).
    // Opens the settings modal — theme + font size, applied live and
    // persisted on every change.
    tree.add_child(
        pane,
        Button::new("\u{2699} Settings")
            .radius(6.0)
            .on_click(|ctx| {
                ctx.push_layer(
                    LayerOptions::modal(),
                    Container::column()
                        .width(360.0)
                        .padding(24.0)
                        .gap(16.0)
                        .background(settings::surface())
                        .radius(12.0),
                    settings::populate_settings_modal,
                );
            }),
    );

    list_cell
}

/// Push one Button per current note into `parent`. Called both at initial
/// build and from `rebuild_children` after add / delete.
fn rebuild_list_into(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    title_sig: Signal<String>,
    body_sig: Signal<String>,
    preview_sig: Signal<bool>,
    list_cell: Rc<Cell<usize>>,
) {
    let ids: Vec<NoteId> = match &state.borrow().phase {
        Phase::Unlocked { notes, .. } => notes.iter().map(|n| n.id).collect(),
        _ => return,
    };

    if ids.is_empty() {
        tree.add_child(
            parent,
            TextWidget::new("No notes yet. Click + New.").color(settings::on_surface_variant()),
        );
        return;
    }

    for id in ids {
        add_row(
            tree,
            parent,
            id,
            Rc::clone(&state),
            title_sig,
            body_sig,
            preview_sig,
            Rc::clone(&list_cell),
        );
    }
}

// Tree-builder for one note row. The argument list is wide because it threads
// the editor's shared note signals (title / body / preview) plus the rebuild
// target alongside the usual tree/parent/state — bundling them into a struct
// would be churn for no real readability win at a single private call site.
#[allow(clippy::too_many_arguments)]
fn add_row(
    tree: &mut WidgetTree,
    parent: usize,
    note_id: NoteId,
    state: Rc<RefCell<AppState>>,
    title_sig: Signal<String>,
    body_sig: Signal<String>,
    preview_sig: Signal<bool>,
    list_cell: Rc<Cell<usize>>,
) {
    // Row container — holds the click-target button and a "✕" delete button
    // side by side. The container reads selection state to flip its bg, so
    // hover and selection coexist without per-button gymnastics.
    let row_state = Rc::clone(&state);
    let row_bg = Reactive::derive(move || {
        let s = row_state.borrow();
        let selected = match &s.phase {
            Phase::Unlocked { selected, .. } => *selected,
            _ => None,
        };
        let theme = settings::current_theme();
        if selected == Some(note_id) {
            theme.colors.primary
        } else {
            theme.colors.surface_variant
        }
    });

    let row = tree.add_child(
        parent,
        Container::row()
            .width_full()
            .gap(4.0)
            .padding(2.0)
            .background(row_bg)
            .radius(4.0),
    );

    let label_state = Rc::clone(&state);
    let click_state = Rc::clone(&state);
    tree.add_child(
        row,
        Button::reactive_label(move || {
            let s = label_state.borrow();
            match &s.phase {
                Phase::Unlocked { notes, .. } => notes
                    .iter()
                    .find(|n| n.id == note_id)
                    .map(|n| {
                        if n.title.is_empty() {
                            "(untitled)".to_string()
                        } else {
                            n.title.clone()
                        }
                    })
                    .unwrap_or_default(),
                _ => String::new(),
            }
        })
        .background(Color::TRANSPARENT)
        .hover_background(settings::hover())
        .radius(4.0)
        // Take the entire remaining row width so the click target spans
        // the full row, not just the label glyphs. Without this the user
        // has to aim precisely at the title text to switch notes —
        // surprising, especially since the row's selection background
        // suggests the whole row is the affordance.
        .grow(1.0)
        .on_click(move |_ctx| {
            select_note(&click_state, note_id, &title_sig, &body_sig, preview_sig);
        }),
    );

    let del_state = Rc::clone(&state);
    let del_cell = list_cell;
    tree.add_child(
        row,
        // Themed red so the destructive action stays legible on both
        // light and dark surfaces (a neutral fill blended into the row's
        // own surface_variant background).
        Button::new("✕")
            .radius(4.0)
            .background(settings::error())
            .on_click(move |ctx| {
                delete_note(&del_state, note_id, &title_sig, &body_sig, preview_sig);
                let parent_idx = del_cell.get();
                let s = Rc::clone(&del_state);
                let c = Rc::clone(&del_cell);
                ctx.rebuild_children(parent_idx, move |tree, parent| {
                    rebuild_list_into(tree, parent, s, title_sig, body_sig, preview_sig, c);
                });
            }),
    );
}

fn select_note(
    state: &Rc<RefCell<AppState>>,
    note_id: NoteId,
    title_sig: &Signal<String>,
    body_sig: &Signal<String>,
    preview_sig: Signal<bool>,
) {
    let snapshot = {
        let s = state.borrow();
        match &s.phase {
            Phase::Unlocked { notes, .. } => notes
                .iter()
                .find(|n| n.id == note_id)
                .map(|n| (n.title.clone(), n.body.clone())),
            _ => None,
        }
    };
    let Some((new_title, new_body)) = snapshot else {
        return;
    };

    {
        let mut s = state.borrow_mut();
        if let Phase::Unlocked { selected, .. } = &mut s.phase {
            *selected = Some(note_id);
        }
    }
    title_sig.set(new_title);
    body_sig.set(new_body);
    // Land on the editor for the newly-selected note rather than carrying the
    // previous note's rendered preview over (the preview subtree is only
    // rebuilt on an explicit edit→preview toggle, so without this it would
    // show stale content).
    preview_sig.set(false);
}

fn create_note(
    state: &Rc<RefCell<AppState>>,
    title_sig: &Signal<String>,
    body_sig: &Signal<String>,
    preview_sig: Signal<bool>,
) -> Option<NoteId> {
    let new_id = {
        let mut s = state.borrow_mut();
        let id = s.next_id;
        s.next_id += 1;
        let Phase::Unlocked {
            notes, selected, ..
        } = &mut s.phase
        else {
            return None;
        };
        notes.push(Note {
            id,
            title: String::new(),
            body: String::new(),
        });
        *selected = Some(id);
        Some(id)
    };
    title_sig.set(String::new());
    body_sig.set(String::new());
    // A fresh note opens in the editor, not in a preview of an empty body.
    preview_sig.set(false);
    // Mark the new (empty) note dirty so the next auto-save tick writes
    // its row to SQLCipher — without this, a brand-new note that the
    // user never edits would silently disappear at lock time (no row
    // ever inserted, lock_and_seal's rewrite would catch it but only
    // for *that* lock cycle).
    if let Some(id) = new_id {
        state.borrow_mut().mark_dirty(id);
    }
    new_id
}

fn delete_note(
    state: &Rc<RefCell<AppState>>,
    note_id: NoteId,
    title_sig: &Signal<String>,
    body_sig: &Signal<String>,
    preview_sig: Signal<bool>,
) {
    // `delete_note_persisted` updates the in-memory vec, drops the
    // row from SQLCipher, and re-selects a sibling if the deleted
    // note was the active one — all atomically under one borrow_mut.
    let was_selected_before = matches!(&state.borrow().phase, Phase::Unlocked { selected, .. } if *selected == Some(note_id));

    if let Err(e) = state.borrow_mut().delete_note_persisted(note_id) {
        eprintln!(
            "knot: failed to delete note {} from storage: {}",
            note_id, e
        );
        return;
    }

    if was_selected_before {
        let payload = {
            let s = state.borrow();
            match &s.phase {
                Phase::Unlocked {
                    notes, selected, ..
                } => selected
                    .and_then(|sel| notes.iter().find(|n| n.id == sel))
                    .map(|n| (n.title.clone(), n.body.clone()))
                    .or(Some((String::new(), String::new()))),
                _ => None,
            }
        };
        if let Some((t, b)) = payload {
            title_sig.set(t);
            body_sig.set(b);
        }
        // The active note changed (deleted note was selected → a sibling, or
        // none, is now current): drop back to the editor so we don't show a
        // stale preview of the note that's gone.
        preview_sig.set(false);
    }
}
