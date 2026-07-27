//! Backlinks panel — "what links here".
//!
//! Shows every note that points at the currently selected one through a
//! `[[wikilink]]`, the reverse of the forward navigation in [`crate::preview`].
//! It is built from the same primitives as the live preview: a
//! [`ReactiveChildren`] keyed on a cheap token rebuilds the list exactly when
//! it can change, and each row is a [`Button`] that navigates to the linking
//! note via the shared [`WikiNav`] (so a backlink click behaves identically to
//! a wikilink click — selection, editor rebase, and tag-chip refresh all
//! follow).
//!
//! ## What counts as a backlink
//!
//! A note *S* backlinks the selected note *T* when some `[[wikilink]]` in *S*'s
//! body resolves (via [`preview::find_note_id_by_title`]) to *T*'s id — i.e.
//! exactly the link a forward click would follow. Resolving through the id (not
//! a raw title match) makes the panel the precise inverse of forward nav, so
//! duplicate titles behave consistently: a `[[Title]]` that forward-nav sends
//! to the *first* note with that title is a backlink of that note only. A note
//! never backlinks itself.
//!
//! ## When it rebuilds
//!
//! The list depends only on the selected note (its id and title) and the set of
//! notes — never on the *selected* note's body, since a note can't be its own
//! backlink and the other notes' bodies are frozen while *T* is the one being
//! edited. So [`backlinks_token`] hashes `(selected id, selected title, note
//! count)`: switching notes, renaming the current note (which changes what
//! `[[old title]]` matched), or adding/deleting a note all rebuild it, while
//! ordinary typing in the body does not.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use sindon::core::Color;
use sindon::reactive::{Reactive, Signal};
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, ReactiveChildren, TextWidget};

use crate::i18n::{self, Key};
use crate::preview::{self, WikiNav};
use crate::settings;
use crate::state::{AppState, Note, NoteId, Phase};
use crate::tag_editor::TagRefresh;

/// Upper bound on the panel's height. A note with an unusually large number of
/// backlinks clips here (with `overflow_hidden`) rather than squeezing the
/// editor above it; in practice backlink counts are small. A scrollable list is
/// a possible follow-up if this ever bites.
const MAX_PANEL_HEIGHT: f32 = 180.0;

/// Build the backlinks panel under `parent` (the editor pane). The panel is the
/// pane's last child, below the edit/preview row, and is hidden whenever no note
/// is selected. `tag_refresh` is threaded into the shared [`WikiNav`] so a
/// backlink click refreshes the editor's tag chips, exactly like a sidebar or
/// wikilink navigation.
pub fn build(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    title_sig: Signal<String>,
    body_sig: Signal<String>,
    tag_refresh: TagRefresh,
) {
    // Reuse the preview's navigation so a backlink click and a wikilink click
    // go through one code path (select + rebase + refresh chips).
    let nav = WikiNav::new(Rc::clone(&state), title_sig, body_sig, tag_refresh);

    // Hidden (display: none) whenever nothing is selected, so the panel takes no
    // space on an empty selection. `note_selected` is a cheap state read; the
    // expensive backlink computation only runs inside the rebuild below.
    let vis_state = Rc::clone(&state);
    let panel = tree.add_child(
        parent,
        Container::column()
            .width_full()
            .gap(4.0)
            // Bound the height and clip the overflow so a pathological backlink
            // count can't eat the editor's space (also pins the flex min-size to
            // 0 — the same `overflow_hidden` trick the preview row uses).
            .max_height(MAX_PANEL_HEIGHT)
            .overflow_hidden()
            .visible(Reactive::derive(move || note_selected(&vis_state))),
    );

    let token_state = Rc::clone(&state);
    tree.add_child(
        panel,
        ReactiveChildren::column().width_full().gap(4.0).source(
            move || backlinks_token(&token_state),
            move |tree, parent| render_panel(tree, parent, &state, &nav),
        ),
    );
}

/// Repopulate the panel for the currently selected note. Renders nothing when
/// there are no backlinks, so the panel collapses to (near) zero height rather
/// than showing a persistent empty-state line.
fn render_panel(
    tree: &mut WidgetTree,
    parent: usize,
    state: &Rc<RefCell<AppState>>,
    nav: &WikiNav,
) {
    let ids = {
        let s = state.borrow();
        match &s.phase {
            Phase::Unlocked {
                notes,
                selected: Some(sel),
                ..
            } => compute_backlinks(notes, *sel),
            _ => Vec::new(),
        }
    };
    if ids.is_empty() {
        return;
    }
    let count = ids.len();

    // Thin rule separating the panel from the editor above it.
    tree.add_child(
        parent,
        Container::row()
            .width_full()
            .height(1.0)
            .background(separator_color()),
    );

    // Header with the count, e.g. "Backlinks (2)". Reactive so a live language
    // swap re-renders it; the count is fixed for this build (it only changes
    // when the list rebuilds anyway).
    tree.add_child(
        parent,
        TextWidget::reactive(move || format!("{} ({count})", i18n::tr(Key::BacklinksTitle)))
            .color(settings::on_surface_variant()),
    );

    for id in ids {
        // Full-width row so the whole strip is the click target (the Button
        // grows to fill it), mirroring the sidebar note rows.
        let row = tree.add_child(parent, Container::row().width_full());
        let nav = nav.clone();
        let label_state = Rc::clone(state);
        tree.add_child(
            row,
            Button::reactive_label(move || note_title_label(&label_state, id))
                .background(Color::TRANSPARENT)
                .hover_background(hover_color())
                .radius(4.0)
                .grow(1.0)
                .on_click(move |ctx| nav.navigate_to_id(id, ctx)),
        );
    }
}

/// Ids of every note that links to `target_id` through a `[[wikilink]]`,
/// in note (storage) order. Excludes `target_id` itself — a note's self-links
/// are not backlinks. Each candidate's wikilink targets are resolved exactly
/// the way a forward click resolves them ([`preview::find_note_id_by_title`]),
/// so the result is the precise reverse of forward navigation.
pub fn compute_backlinks(notes: &[Note], target_id: NoteId) -> Vec<NoteId> {
    notes
        .iter()
        .filter(|src| src.id != target_id)
        // A trashed note isn't a live backlink source — it shouldn't show up
        // under "what links here" until it's restored. (The target side is
        // already excluded by `find_note_id_by_title`, which skips trash.)
        .filter(|src| src.deleted_at.is_none())
        .filter(|src| {
            preview::wikilink_targets(&src.body)
                .iter()
                .any(|t| preview::find_note_id_by_title(notes, t) == Some(target_id))
        })
        .map(|src| src.id)
        .collect()
}

/// Change token for the backlinks [`ReactiveChildren`]. See the module note on
/// why the selected note's *body* is deliberately excluded.
fn backlinks_token(state: &Rc<RefCell<AppState>>) -> u64 {
    let mut h = DefaultHasher::new();
    let s = state.borrow();
    match &s.phase {
        Phase::Unlocked {
            notes, selected, ..
        } => {
            selected.hash(&mut h);
            if let Some(sel) = selected {
                if let Some(note) = notes.iter().find(|n| n.id == *sel) {
                    note.title.hash(&mut h);
                }
            }
            notes.len().hash(&mut h);
        }
        // Any non-unlocked phase hashes to a single stable value — the panel is
        // hidden then anyway.
        _ => 0u8.hash(&mut h),
    }
    h.finish()
}

/// A note is selected (and the app unlocked) — the condition for showing the
/// panel at all. A cheap state read suitable for a per-paint visibility derive.
fn note_selected(state: &Rc<RefCell<AppState>>) -> bool {
    matches!(
        &state.borrow().phase,
        Phase::Unlocked {
            selected: Some(_),
            ..
        }
    )
}

/// Display label for a backlink row: the linking note's current title, or the
/// localized "(untitled)" fallback for an empty one. Read live (by id) so a
/// rename or language swap updates the row without a rebuild; an empty string
/// for a since-deleted id leaves a blank, harmless row (its click no-ops).
fn note_title_label(state: &Rc<RefCell<AppState>>, id: NoteId) -> String {
    let s = state.borrow();
    match &s.phase {
        Phase::Unlocked { notes, .. } => notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| {
                if n.title.trim().is_empty() {
                    i18n::tr(Key::Untitled).to_string()
                } else {
                    n.title.clone()
                }
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn separator_color() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().colors.input_border)
}

fn hover_color() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().hover.bg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: NoteId, title: &str, body: &str) -> Note {
        Note {
            id,
            title: title.to_string(),
            body: body.to_string(),
            tags: Vec::new(),
            pinned: false,
            deleted_at: None,
        }
    }

    #[test]
    fn finds_notes_that_link_to_the_target() {
        let notes = vec![
            note(1, "Home", "see [[Projects]] and [[Ideas]]"),
            note(2, "Projects", "no links here"),
            note(3, "Ideas", "back to [[Projects]]"),
        ];
        // Projects (id 2) is linked from Home and Ideas.
        assert_eq!(compute_backlinks(&notes, 2), vec![1, 3]);
        // Ideas (id 3) is linked only from Home.
        assert_eq!(compute_backlinks(&notes, 3), vec![1]);
        // Home (id 1) has no inbound links.
        assert!(compute_backlinks(&notes, 1).is_empty());
    }

    #[test]
    fn alias_links_count_and_self_links_do_not() {
        let notes = vec![
            // Aliased link `[[Target|shown text]]` still resolves to Target.
            note(1, "Source", "jump to [[Target|the target]]"),
            // A self-reference must not make a note its own backlink.
            note(2, "Target", "I mention [[Target]] myself"),
        ];
        assert_eq!(compute_backlinks(&notes, 2), vec![1]);
    }

    #[test]
    fn title_match_is_ascii_case_insensitive() {
        let notes = vec![note(1, "Source", "link to [[home]]"), note(2, "Home", "")];
        // "home" resolves to "Home" (ASCII case-folded), so it's a backlink.
        assert_eq!(compute_backlinks(&notes, 2), vec![1]);
    }

    #[test]
    fn duplicate_titles_resolve_to_the_first_only() {
        // Two notes share the title "Dup"; a `[[Dup]]` link forward-navigates to
        // the first (id 2), so it's a backlink of id 2 but not id 3.
        let notes = vec![
            note(1, "Linker", "see [[Dup]]"),
            note(2, "Dup", ""),
            note(3, "Dup", ""),
        ];
        assert_eq!(compute_backlinks(&notes, 2), vec![1]);
        assert!(compute_backlinks(&notes, 3).is_empty());
    }

    #[test]
    fn dangling_and_codey_brackets_do_not_match() {
        let notes = vec![
            // A dangling `[[` (no closer) and an empty `[[]]` are not wikilinks.
            note(1, "A", "broken [[ and empty [[]] here"),
            note(2, "B", ""),
        ];
        assert!(compute_backlinks(&notes, 2).is_empty());
    }
}
