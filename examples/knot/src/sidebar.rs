//! Sidebar — header with "+ New", a tag-filter chip row, and a scrollable
//! note list.
//!
//! The list is a `ScrollView > Container::column`. Each row is a `Button`
//! whose label and background are reactive — title edits in the editor
//! reflect live, and the selected row stays highlighted across redraws. The
//! list is narrowed by the tag filter above it: clicking a filter chip
//! toggles that tag into [`AppState`]'s filter set (intersection — see
//! `state::note_matches_filter`) and rebuilds the list.
//!
//! Mutations that change the row set (add / delete) or the available tags
//! rebuild the affected subtree via `EventContext::rebuild_children`. The two
//! parent indices (note list, filter row) are stashed in `Rc<Cell<usize>>`
//! inside [`SidebarWiring`] because the closures that trigger rebuilds capture
//! the wiring at build time and only learn the real indices once the parent
//! widgets are inserted into the tree.
//!
//! [`SidebarRefresh`] is the editor → sidebar counterpart of
//! [`TagRefresh`]: the editor fires it after a
//! tag is added or removed so the filter chips track the vault's tag set,
//! since the editor can't reach this subtree directly and there is no
//! reactive-children primitive to do it declaratively.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shroud::core::Color;
use shroud::reactive::{Reactive, Signal};
use shroud::widgets::layer::LayerOptions;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, EventContext, ScrollView, TextWidget};

use crate::settings;
use crate::state::{AppState, Note, NoteId, Phase};
use crate::tag_editor::TagRefresh;

const SIDEBAR_WIDTH: f32 = 260.0;

/// The rebuild closure [`SidebarRefresh`] stores.
type RefreshFn = Rc<dyn Fn(&mut EventContext)>;

/// Editor → sidebar bridge: re-renders the filter chip row and the note list
/// for the current vault tag set. The sidebar installs the real closure in
/// [`build`]; the editor fires it whenever a tag is added or removed (see
/// `tag_editor`), since it can't reach this subtree and there is no
/// reactive-children primitive. Mirror of
/// [`TagRefresh`], which runs the other way.
#[derive(Clone, Default)]
pub struct SidebarRefresh {
    inner: Rc<RefCell<Option<RefreshFn>>>,
}

impl SidebarRefresh {
    pub fn new() -> Self {
        Self::default()
    }

    fn install(&self, f: RefreshFn) {
        *self.inner.borrow_mut() = Some(f);
    }

    /// Re-render the filter row + note list for the current tag set. No-op
    /// until [`build`] has installed the closure. Clones the closure out
    /// before calling it so the `RefCell` borrow isn't held across the
    /// rebuild.
    pub fn fire(&self, ctx: &mut EventContext) {
        let f = self.inner.borrow().clone();
        if let Some(f) = f {
            f(ctx);
        }
    }
}

/// The shared bits every sidebar rebuild helper needs: app state, the editor's
/// note signals (so selecting a note rebases the editor inputs), the two
/// rebuild-target indices (note list + filter row), and the editor-chip
/// refresh bridge. Cheap to clone — `Signal` is `Copy` and the rest is `Rc`.
///
/// (An earlier single-row builder threaded these as explicit args, noting a
/// struct wasn't worth it at one call site. The tag filter added several more
/// rebuild sites — chip toggles, Clear, the refresh bridge — so the shared
/// struct now earns its keep, matching `tag_editor`'s `Wiring`.)
#[derive(Clone)]
struct SidebarWiring {
    state: Rc<RefCell<AppState>>,
    title: Signal<String>,
    body: Signal<String>,
    preview: Signal<bool>,
    list: Rc<Cell<usize>>,
    filter: Rc<Cell<usize>>,
    tag_refresh: TagRefresh,
}

/// Build the whole sidebar pane. The wide signature mirrors `editor::build`
/// (the two panes share the editor signals + both refresh bridges); they're
/// bundled into [`SidebarWiring`] immediately inside.
#[allow(clippy::too_many_arguments)]
pub fn build(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    title_sig: Signal<String>,
    body_sig: Signal<String>,
    preview_sig: Signal<bool>,
    tag_refresh: TagRefresh,
    sidebar_refresh: SidebarRefresh,
) {
    let w = SidebarWiring {
        state,
        title: title_sig,
        body: body_sig,
        preview: preview_sig,
        list: Rc::new(Cell::new(0)),
        filter: Rc::new(Cell::new(0)),
        tag_refresh,
    };

    let pane = tree.add_child(
        parent,
        Container::column()
            .width(SIDEBAR_WIDTH)
            .height_full()
            .padding(16.0)
            .gap(12.0)
            .background(settings::surface()),
    );

    // Header: title on the left, "+ New" pushed to the right edge.
    let header = tree.add_child(pane, Container::row().gap(8.0).align_center());
    tree.add_child(header, TextWidget::new("Knot").font_size(20.0));
    tree.add_child(header, Container::row().grow(1.0));
    {
        let w = w.clone();
        tree.add_child(
            header,
            Button::new("+ New").radius(6.0).on_click(move |ctx| {
                if create_note(&w.state, &w.title, &w.body, w.preview).is_some() {
                    // A new note has no tags, so an active filter would hide it
                    // the instant it's created — clear the filter so the note
                    // the user just asked for is actually visible in the list.
                    w.state.borrow_mut().clear_filter();
                    rebuild_sidebar(&w, ctx);
                    // The new note is selected and tagless — refresh the
                    // editor's chip row to match.
                    w.tag_refresh.fire(ctx);
                }
            }),
        );
    }

    // Tag-filter chip row, between the header and the list. Hidden entirely
    // until the vault has at least one tag, so an untagged vault shows no
    // empty strip; it wraps once a vault accumulates many tags.
    let filter_visible_state = Rc::clone(&w.state);
    let filter_row = tree.add_child(
        pane,
        Container::row()
            .width_full()
            .flex_wrap(true)
            .gap(6.0)
            .align_center()
            .visible(Reactive::derive(move || {
                filter_visible_state.borrow().has_any_tags()
            })),
    );
    w.filter.set(filter_row);

    // Note list.
    let scroll = tree.add_child(pane, ScrollView::new().width_full().grow(1.0));
    let list = tree.add_child(scroll, Container::column().width_full().gap(4.0));
    w.list.set(list);

    populate_filter(tree, filter_row, &w);
    populate_list(tree, list, &w);

    // Install the editor → sidebar refresh: re-render the filter chips + list
    // for the current tag set (`rebuild_sidebar` prunes orphaned filters).
    {
        let w = w.clone();
        sidebar_refresh.install(Rc::new(move |ctx: &mut EventContext| {
            rebuild_sidebar(&w, ctx);
        }));
    }

    // Pinned to the bottom of the pane (the scroll above takes grow:1).
    // Opens the settings modal — theme + font size + auto-lock, applied live
    // and persisted on every change.
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
}

/// Rebuild both dynamic sidebar subtrees — the filter chip row and the note
/// list. Used after add / delete and by the editor refresh bridge, where the
/// available tags *and* the visible rows can both change. Prunes filter tags
/// that no longer exist first, so a delete / editor-removal that strips the
/// last instance of a filtered tag doesn't leave the list stuck on it.
fn rebuild_sidebar(w: &SidebarWiring, ctx: &mut EventContext) {
    w.state.borrow_mut().prune_filter();
    rebuild_filter(w, ctx);
    rebuild_list(w, ctx);
}

fn rebuild_filter(w: &SidebarWiring, ctx: &mut EventContext) {
    let parent = w.filter.get();
    let w = w.clone();
    ctx.rebuild_children(parent, move |tree, parent| {
        populate_filter(tree, parent, &w)
    });
}

fn rebuild_list(w: &SidebarWiring, ctx: &mut EventContext) {
    let parent = w.list.get();
    let w = w.clone();
    ctx.rebuild_children(parent, move |tree, parent| populate_list(tree, parent, &w));
}

/// One toggle chip per distinct vault tag, plus a trailing "Clear" that wipes
/// the whole filter. A chip is highlighted while its tag is in the active
/// filter; clicking it toggles membership and rebuilds the list. The chip's
/// own highlight and Clear's visibility track the filter reactively, so only
/// the list needs an imperative rebuild on a toggle.
fn populate_filter(tree: &mut WidgetTree, parent: usize, w: &SidebarWiring) {
    let tags = w.state.borrow().all_tags();
    if tags.is_empty() {
        return;
    }

    for tag in tags {
        // Active = highlighted like the settings modal's selected option:
        // primary fill + on_primary text, else the muted surface_variant pair.
        let bg_state = Rc::clone(&w.state);
        let bg_tag = tag.clone();
        let bg = Reactive::derive(move || {
            let t = settings::current_theme();
            if bg_state.borrow().is_filter_active(&bg_tag) {
                t.colors.primary
            } else {
                t.colors.surface_variant
            }
        });
        let fg_state = Rc::clone(&w.state);
        let fg_tag = tag.clone();
        let fg = Reactive::derive(move || {
            let t = settings::current_theme();
            if fg_state.borrow().is_filter_active(&fg_tag) {
                t.colors.on_primary
            } else {
                t.colors.on_surface
            }
        });

        let w = w.clone();
        let value = tag.clone();
        tree.add_child(
            parent,
            Button::new(tag)
                .font_size(13.0)
                .radius(10.0)
                .background(bg)
                .text_color(fg)
                .on_click(move |ctx| {
                    w.state.borrow_mut().toggle_filter_tag(&value);
                    // Membership (which chips exist) is unchanged by a toggle,
                    // so only the list needs rebuilding; the chip highlight and
                    // Clear's visibility update reactively.
                    rebuild_list(&w, ctx);
                }),
        );
    }

    // Clear-all. Always added, shown only while a filter is active so it
    // doesn't clutter the row when nothing is selected.
    let clear_visible_state = Rc::clone(&w.state);
    let w = w.clone();
    tree.add_child(
        parent,
        Button::new("Clear")
            .font_size(13.0)
            .radius(10.0)
            .background(Color::TRANSPARENT)
            .hover_background(settings::hover())
            .text_color(settings::on_surface_variant())
            .visible(Reactive::derive(move || {
                clear_visible_state.borrow().is_filtering()
            }))
            .on_click(move |ctx| {
                w.state.borrow_mut().clear_filter();
                rebuild_list(&w, ctx);
            }),
    );
}

/// Push one Button per visible note into `parent`. Visible = matches the
/// active tag filter. Called at initial build and from `rebuild_children`
/// after add / delete / filter changes.
fn populate_list(tree: &mut WidgetTree, parent: usize, w: &SidebarWiring) {
    let (ids, total) = {
        let s = w.state.borrow();
        (s.filtered_note_ids(), s.note_count())
    };

    if total == 0 {
        tree.add_child(
            parent,
            TextWidget::new("No notes yet. Click + New.").color(settings::on_surface_variant()),
        );
        return;
    }
    if ids.is_empty() {
        // Notes exist but the active filter hid them all — distinct from the
        // empty-vault message so the user knows it's the filter, not an empty
        // vault.
        tree.add_child(
            parent,
            TextWidget::new("No notes match the selected tags.")
                .color(settings::on_surface_variant()),
        );
        return;
    }

    for id in ids {
        add_row(tree, parent, id, w);
    }
}

// Tree-builder for one note row. The row container holds the click-target
// button and a "✕" delete button side by side and reads selection state to
// flip its background, so hover and selection coexist without per-button
// gymnastics.
fn add_row(tree: &mut WidgetTree, parent: usize, note_id: NoteId, w: &SidebarWiring) {
    let row_state = Rc::clone(&w.state);
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

    let label_state = Rc::clone(&w.state);
    {
        let w = w.clone();
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
            .on_click(move |ctx| {
                select_note(&w.state, note_id, &w.title, &w.body, w.preview);
                // Re-render the editor's chips for the newly selected note.
                w.tag_refresh.fire(ctx);
            }),
        );
    }

    let w = w.clone();
    tree.add_child(
        row,
        // Themed red so the destructive action stays legible on both light
        // and dark surfaces.
        Button::new("✕")
            .radius(4.0)
            .background(settings::error())
            .on_click(move |ctx| {
                delete_note(&w.state, note_id, &w.title, &w.body, w.preview);
                // The row set changed, and the deleted note's tags may have
                // been the last of their kind — rebuild the list + filter row
                // (the latter prunes any now-orphaned filter tags).
                rebuild_sidebar(&w, ctx);
                // Selection may have moved to a sibling (or none) — sync the
                // editor's chip row to whatever note is active now.
                w.tag_refresh.fire(ctx);
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
            tags: Vec::new(),
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
