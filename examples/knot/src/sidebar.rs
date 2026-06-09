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
use shroud::platform::FileDialog;
use shroud::reactive::{Reactive, Signal};
use shroud::widgets::layer::LayerOptions;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, EventContext, Input, ScrollView, TextWidget};

use crate::i18n::{self, Key};
use crate::notice;
use crate::settings;
use crate::settings::SortMode;
use crate::state::{AppState, NoteId, Phase};
use crate::tag_editor::TagRefresh;

const SIDEBAR_WIDTH: f32 = 260.0;

/// Upper bound on a file the Import button will read into a note. Guards
/// against accidentally slurping a multi-hundred-MB file into a text buffer;
/// ordinary markdown notes sit far below this.
const MAX_IMPORT_BYTES: u64 = 8 * 1024 * 1024;

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
    list: Rc<Cell<usize>>,
    filter: Rc<Cell<usize>>,
    /// Bound value of the search box. Kept so `+ New` can clear the field
    /// (and the underlying query) when it drops the filter, so a freshly
    /// created note isn't hidden by a stale search.
    search: Signal<String>,
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
    tag_refresh: TagRefresh,
    sidebar_refresh: SidebarRefresh,
) {
    let w = SidebarWiring {
        state,
        title: title_sig,
        body: body_sig,
        list: Rc::new(Cell::new(0)),
        filter: Rc::new(Cell::new(0)),
        search: Signal::new(String::new()),
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

    // Header: title on the left, then the Import + "+ New" buttons pushed to
    // the right edge.
    let header = tree.add_child(pane, Container::row().gap(8.0).align_center());
    // Pin the brand title with `shrink(0)` so it keeps its natural width. The
    // header is a fixed-width row (the 260 px sidebar); without this, wider
    // trailing buttons — notably the longer Japanese labels — crowd the row
    // and flex-shrink compresses the title until "Knot" breaks mid-word
    // ("kno⏎t"). A text widget can't carry flex props itself, so it rides in a
    // non-shrinking wrapper (same pattern as the body Input's grow wrapper).
    let title_box = tree.add_child(header, Container::row().shrink(0.0));
    tree.add_child(title_box, TextWidget::new("Knot").font_size(20.0));
    tree.add_child(header, Container::row().grow(1.0));

    // Import (secondary): read a .md/.txt file into a new note. Sits left of
    // the primary "+ New" and is styled muted so "+ New" stays the focal action.
    {
        let w = w.clone();
        tree.add_child(
            header,
            Button::reactive_label(|| i18n::tr(Key::SidebarImport).to_string())
                .radius(6.0)
                .background(Color::TRANSPARENT)
                .hover_background(settings::hover())
                .text_color(settings::on_surface_variant())
                .on_click(move |ctx| import_note(&w, ctx)),
        );
    }

    {
        let w = w.clone();
        tree.add_child(
            header,
            Button::reactive_label(|| i18n::tr(Key::SidebarNewNote).to_string())
                .radius(6.0)
                .on_click(move |ctx| {
                    if create_note(&w.state, &w.title, &w.body).is_some() {
                        // A new note has no tags, so an active filter would hide it
                        // the instant it's created — clear the filter so the note
                        // the user just asked for is actually visible in the list.
                        w.state.borrow_mut().clear_filter();
                        // An active search would likewise hide the new empty note
                        // (empty title + body match no non-empty query) — clear the
                        // query and the search box along with the filter.
                        w.state.borrow_mut().clear_search();
                        w.search.set(String::new());
                        rebuild_sidebar(&w, ctx);
                        // The new note is selected and tagless — refresh the
                        // editor's chip row to match.
                        w.tag_refresh.fire(ctx);
                    }
                }),
        );
    }

    // Search box, between the header and the tag filter. Always visible while
    // unlocked; typing narrows the list (title + body substring, intersected
    // with the tag filter — see `state::note_matches_search`). Its tree index
    // is recorded on the app state so the global Ctrl+F shortcut (wired in
    // `main`) can focus it from anywhere, including mid-edit.
    {
        let w_change = w.clone();
        let search_input = tree.add_child(
            pane,
            Input::new()
                .placeholder(i18n::tr(Key::SidebarSearchPlaceholder))
                .value(w.search)
                .font_size(13.0)
                .on_change(move |val, ctx| {
                    w_change.state.borrow_mut().set_search_query(val);
                    rebuild_list(&w_change, ctx);
                }),
        );
        w.state.borrow_mut().search_input_idx = Some(search_input);
    }

    // Sort-order row, between the search box and the tag filter. Picks how the
    // list below is ordered (pinned notes always lead — see
    // `state::compare_notes`). Hidden until the vault has at least one note, so
    // an empty vault shows no orphan control. Each button persists the choice
    // and rebuilds the list to re-sort it; the active one highlights reactively.
    let sort_visible_state = Rc::clone(&w.state);
    let sort_row = tree.add_child(
        pane,
        Container::row()
            .gap(6.0)
            .align_center()
            .visible(Reactive::derive(move || {
                sort_visible_state.borrow().note_count() > 0
            })),
    );
    tree.add_child(
        sort_row,
        TextWidget::reactive(|| i18n::tr(Key::SidebarSort).to_string())
            .font_size(13.0)
            .color(settings::on_surface_variant()),
    );
    for mode in [SortMode::Created, SortMode::TitleAsc, SortMode::TitleDesc] {
        let bg = Reactive::derive(move || {
            let t = settings::current_theme();
            if settings::current_sort() == mode {
                t.colors.primary
            } else {
                t.colors.surface_variant
            }
        });
        let fg = Reactive::derive(move || {
            let t = settings::current_theme();
            if settings::current_sort() == mode {
                t.colors.on_primary
            } else {
                t.colors.on_surface
            }
        });
        let w = w.clone();
        tree.add_child(
            sort_row,
            Button::reactive_label(move || i18n::tr(mode.key()).to_string())
                .font_size(13.0)
                .radius(10.0)
                .background(bg)
                .text_color(fg)
                .on_click(move |ctx| {
                    settings::signals().sort.set(mode);
                    settings::persist();
                    // Order changed → re-sort the visible rows. The highlight
                    // tracks `current_sort` reactively, so only the list needs
                    // an imperative rebuild.
                    rebuild_list(&w, ctx);
                }),
        );
    }

    // Tag-filter chip row, between the search box and the list. Hidden entirely
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
    // and persisted on every change, plus the change-password entry (which is
    // why the modal needs the app state).
    let settings_state = Rc::clone(&w.state);
    tree.add_child(
        pane,
        Button::reactive_label(|| i18n::tr(Key::SidebarSettings).to_string())
            .radius(6.0)
            .on_click(move |ctx| {
                let st = Rc::clone(&settings_state);
                ctx.push_layer(
                    LayerOptions::modal(),
                    Container::column()
                        .width(360.0)
                        .padding(24.0)
                        .gap(16.0)
                        .background(settings::surface())
                        .radius(12.0),
                    move |tree, dialog| settings::populate_settings_modal(tree, dialog, st),
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
        Button::reactive_label(|| i18n::tr(Key::SidebarClear).to_string())
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
    let (ids, total, searching, filtering) = {
        let s = w.state.borrow();
        (
            s.filtered_note_ids(settings::current_sort()),
            s.note_count(),
            s.is_searching(),
            s.is_filtering(),
        )
    };

    if total == 0 {
        tree.add_child(
            parent,
            TextWidget::reactive(|| i18n::tr(Key::SidebarNoNotesYet).to_string())
                .color(settings::on_surface_variant()),
        );
        return;
    }
    if ids.is_empty() {
        // Notes exist but the active search and/or tag filter hid them all —
        // name whichever is active so the user knows it's a narrowing, not an
        // empty vault.
        let key = match (searching, filtering) {
            (true, true) => Key::SidebarNoMatchSearchTags,
            (true, false) => Key::SidebarNoMatchSearch,
            _ => Key::SidebarNoMatchTags,
        };
        tree.add_child(
            parent,
            TextWidget::reactive(move || i18n::tr(key).to_string())
                .color(settings::on_surface_variant()),
        );
        return;
    }

    for id in ids {
        add_row(tree, parent, id, w);
    }
}

/// Hover background for a row's transparent buttons (pin + title), aware of
/// the row's selection state. The generic `hover.bg` token is an opaque
/// "raised row" grey tuned for the unselected (grey) rows; painted over a
/// *selected* row it would bury the primary fill under grey — reads like a
/// dark hole punched in the selection. So a selected row hovers to
/// `primary_hover` (a brighter shade of its own blue) instead, which lifts the
/// selection rather than covering it. Re-derived each paint, so it tracks both
/// selection changes and a live theme swap.
fn row_hover_bg(state: &Rc<RefCell<AppState>>, note_id: NoteId) -> Reactive<Color> {
    let state = Rc::clone(state);
    Reactive::derive(move || {
        let theme = settings::current_theme();
        let selected = matches!(
            &state.borrow().phase,
            Phase::Unlocked { selected, .. } if *selected == Some(note_id)
        );
        if selected {
            theme.colors.primary_hover
        } else {
            theme.hover.bg
        }
    })
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

    // Pin toggle (leftmost). A filled star marks a pinned note, an outline an
    // unpinned one; clicking flips it and rebuilds the list so the row floats
    // to (or drops from) the top immediately. Star glyphs (U+2605/2606) render
    // in the text font, unlike astral-plane pin emoji.
    let pin_label_state = Rc::clone(&w.state);
    let pin_color_state = Rc::clone(&w.state);
    {
        let w = w.clone();
        tree.add_child(
            row,
            Button::reactive_label(move || {
                if pin_label_state.borrow().is_pinned(note_id) {
                    "\u{2605}".to_string()
                } else {
                    "\u{2606}".to_string()
                }
            })
            .radius(4.0)
            .background(Color::TRANSPARENT)
            .hover_background(row_hover_bg(&w.state, note_id))
            .text_color(Reactive::derive(move || {
                let t = settings::current_theme();
                if pin_color_state.borrow().is_pinned(note_id) {
                    t.colors.primary
                } else {
                    t.colors.on_surface_variant
                }
            }))
            .on_click(move |ctx| {
                w.state.borrow_mut().toggle_pin(note_id);
                // Pin state changed the sort order — re-sort the visible rows.
                rebuild_list(&w, ctx);
            }),
        );
    }

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
                                i18n::tr(Key::Untitled).to_string()
                            } else {
                                n.title.clone()
                            }
                        })
                        .unwrap_or_default(),
                    _ => String::new(),
                }
            })
            .background(Color::TRANSPARENT)
            .hover_background(row_hover_bg(&w.state, note_id))
            .radius(4.0)
            // Take the entire remaining row width so the click target spans
            // the full row, not just the label glyphs. Without this the user
            // has to aim precisely at the title text to switch notes —
            // surprising, especially since the row's selection background
            // suggests the whole row is the affordance.
            .grow(1.0)
            .on_click(move |ctx| {
                select_note(&w.state, note_id, &w.title, &w.body);
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
                delete_note(&w.state, note_id, &w.title, &w.body);
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
    // No preview reset needed: the live preview pane keys off the body, so
    // setting `body_sig` above already re-renders it for the new note.
}

fn create_note(
    state: &Rc<RefCell<AppState>>,
    title_sig: &Signal<String>,
    body_sig: &Signal<String>,
) -> Option<NoteId> {
    // `add_note` allocates the id, selects the note, and marks it dirty so the
    // next auto-save tick writes its row to SQLCipher — without that, a
    // brand-new note the user never edits would never get a row inserted.
    let new_id = state.borrow_mut().add_note(String::new(), String::new());
    if new_id.is_some() {
        // Rebase the editor inputs onto the new (empty) note. The live preview
        // tracks the body signal, so clearing it already shows the empty note.
        title_sig.set(String::new());
        body_sig.set(String::new());
    }
    new_id
}

/// Import a single `.md`/`.txt` file as a new note: the file contents become
/// the body and the file name (stem) becomes the title — the inverse of
/// Export (`editor::export_selected`), which writes the body verbatim to
/// `<title>.md`. Tags don't round-trip (Export never writes them, so there's
/// nothing to read back). Clears any active filter/search so the freshly
/// imported, tagless note is visible, mirroring `+ New`.
fn import_note(w: &SidebarWiring, ctx: &mut EventContext) {
    let Some(path) = FileDialog::new()
        .title(i18n::tr(Key::DialogImportNote))
        .filter("Markdown / text", &["md", "markdown", "txt"])
        .open_file()
    else {
        return;
    };

    // Refuse a pathologically large file before reading it into memory.
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > MAX_IMPORT_BYTES {
            notice::show(
                i18n::tr(Key::ErrImportTooLarge)
                    .replace("{n}", &(MAX_IMPORT_BYTES / (1024 * 1024)).to_string()),
            );
            return;
        }
    }

    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            notice::show(format!("{}{e}", i18n::tr(Key::ErrReadFilePrefix)));
            return;
        }
    };
    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported")
        .to_string();

    if w.state
        .borrow_mut()
        .add_note(title.clone(), body.clone())
        .is_none()
    {
        return;
    }
    // Rebase the editor onto the imported note.
    w.title.set(title);
    w.body.set(body);
    // A tagless new note would be hidden by an active filter/search — clear
    // both (and the search box) so the import is actually visible.
    {
        let mut s = w.state.borrow_mut();
        s.clear_filter();
        s.clear_search();
    }
    w.search.set(String::new());
    rebuild_sidebar(w, ctx);
    // The newly selected note is tagless — refresh the editor's chip row.
    w.tag_refresh.fire(ctx);
}

fn delete_note(
    state: &Rc<RefCell<AppState>>,
    note_id: NoteId,
    title_sig: &Signal<String>,
    body_sig: &Signal<String>,
) {
    // `delete_note_persisted` updates the in-memory vec, drops the
    // row from SQLCipher, and re-selects a sibling if the deleted
    // note was the active one — all atomically under one borrow_mut.
    let was_selected_before = matches!(&state.borrow().phase, Phase::Unlocked { selected, .. } if *selected == Some(note_id));

    if let Err(e) = state.borrow_mut().delete_note_persisted(note_id) {
        notice::show(format!("{}{e}", i18n::tr(Key::ErrDeleteNotePrefix)));
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
        // Setting `body_sig` above re-renders the live preview for whatever
        // note is current now, so the stale preview of the deleted note can't
        // linger — no explicit reset required.
    }
}
