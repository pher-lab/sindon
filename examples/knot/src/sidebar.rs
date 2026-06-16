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

    // View toggle: live notes vs trash. The active view highlights like the
    // sort / filter chips; switching rebuilds the list (and, via reactive
    // visibility, hides the search / sort / filter rows in the trash view). The
    // "Empty trash" action sits at the right, shown only in a non-empty trash.
    let view_row = tree.add_child(pane, Container::row().gap(6.0).align_center());
    for trash in [false, true] {
        let bg_state = Rc::clone(&w.state);
        let bg = Reactive::derive(move || {
            let t = settings::current_theme();
            if bg_state.borrow().is_trash_view() == trash {
                t.colors.primary
            } else {
                t.colors.surface_variant
            }
        });
        let fg_state = Rc::clone(&w.state);
        let fg = Reactive::derive(move || {
            let t = settings::current_theme();
            if fg_state.borrow().is_trash_view() == trash {
                t.colors.on_primary
            } else {
                t.colors.on_surface
            }
        });
        let label_state = Rc::clone(&w.state);
        let w_btn = w.clone();
        tree.add_child(
            view_row,
            Button::reactive_label(move || {
                if trash {
                    // "Trash (n)" — the count tracks the bin reactively.
                    format!(
                        "{} ({})",
                        i18n::tr(Key::SidebarViewTrash),
                        label_state.borrow().trash_count()
                    )
                } else {
                    i18n::tr(Key::SidebarViewNotes).to_string()
                }
            })
            .font_size(13.0)
            .radius(10.0)
            .background(bg)
            .text_color(fg)
            .on_click(move |ctx| {
                w_btn.state.borrow_mut().set_trash_view(trash);
                rebuild_sidebar(&w_btn, ctx);
            }),
        );
    }
    tree.add_child(view_row, Container::row().grow(1.0));
    {
        let vis_state = Rc::clone(&w.state);
        let w_empty = w.clone();
        tree.add_child(
            view_row,
            Button::reactive_label(|| i18n::tr(Key::TrashEmptyBtn).to_string())
                .font_size(13.0)
                .radius(6.0)
                .background(settings::error())
                .visible(Reactive::derive(move || {
                    let s = vis_state.borrow();
                    s.is_trash_view() && s.trash_count() > 0
                }))
                .on_click(move |ctx| empty_trash(&w_empty, ctx)),
        );
    }

    // Search box, between the view toggle and the tag filter. Hidden in the
    // trash view (search narrows the live list only). Typing narrows the list
    // (title + body substring, intersected with the tag filter — see
    // `state::note_matches_search`). Its tree index is recorded on the app
    // state so the global Ctrl+F shortcut (wired in `main`) can focus it from
    // anywhere, including mid-edit. The Input rides in a full-width column
    // wrapper because `Input` itself carries no `.visible()` builder.
    {
        let w_change = w.clone();
        let search_vis = Rc::clone(&w.state);
        let search_wrap = tree.add_child(
            pane,
            Container::column()
                .width_full()
                .visible(Reactive::derive(move || {
                    !search_vis.borrow().is_trash_view()
                })),
        );
        let search_input = tree.add_child(
            search_wrap,
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
                let s = sort_visible_state.borrow();
                !s.is_trash_view() && s.live_note_count() > 0
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
                let s = filter_visible_state.borrow();
                !s.is_trash_view() && s.has_any_tags()
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
    let (ids, trash_view, live_total, searching, filtering) = {
        let s = w.state.borrow();
        (
            s.filtered_note_ids(settings::current_sort()),
            s.is_trash_view(),
            s.live_note_count(),
            s.is_searching(),
            s.is_filtering(),
        )
    };

    // Trash view: just the soft-deleted notes (search / tag filter don't apply),
    // or an "empty" line when the bin is clear.
    if trash_view {
        if ids.is_empty() {
            tree.add_child(
                parent,
                TextWidget::reactive(|| i18n::tr(Key::TrashEmptyHint).to_string())
                    .color(settings::on_surface_variant()),
            );
            return;
        }
        for id in ids {
            add_row(tree, parent, id, w, true);
        }
        return;
    }

    // Live view.
    if live_total == 0 {
        tree.add_child(
            parent,
            TextWidget::reactive(|| i18n::tr(Key::SidebarNoNotesYet).to_string())
                .color(settings::on_surface_variant()),
        );
        return;
    }
    if ids.is_empty() {
        // Live notes exist but the active search and/or tag filter hid them all
        // — name whichever is active so the user knows it's a narrowing, not an
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
        add_row(tree, parent, id, w, false);
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

// Tree-builder for one note row. The row container reads selection state to
// flip its background, so hover and selection coexist without per-button
// gymnastics. The trailing actions depend on the view: a live row carries a
// pin toggle and a "✕" that moves the note to the trash; a trash row carries a
// "Restore" and a "✕" that permanently deletes it (behind a confirmation).
fn add_row(tree: &mut WidgetTree, parent: usize, note_id: NoteId, w: &SidebarWiring, trash: bool) {
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

    // Pin toggle (leftmost) — live view only; pinning is meaningless in the
    // trash. A filled star marks a pinned note, an outline an unpinned one;
    // clicking flips it and rebuilds the list so the row floats to (or drops
    // from) the top immediately. Star glyphs (U+2605/2606) render in the text
    // font, unlike astral-plane pin emoji.
    if !trash {
        let pin_label_state = Rc::clone(&w.state);
        let pin_color_state = Rc::clone(&w.state);
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

    if trash {
        // Restore: pull the note back into the live list.
        {
            let w = w.clone();
            tree.add_child(
                row,
                Button::reactive_label(|| i18n::tr(Key::TrashRestore).to_string())
                    .font_size(13.0)
                    .radius(4.0)
                    .background(Color::TRANSPARENT)
                    .hover_background(row_hover_bg(&w.state, note_id))
                    .text_color(settings::on_surface())
                    .on_click(move |ctx| {
                        w.state.borrow_mut().restore_note(note_id);
                        // The note rejoins the live list (and may bring a tag
                        // back) — rebuild both subtrees and refresh the editor.
                        rebuild_sidebar(&w, ctx);
                        w.tag_refresh.fire(ctx);
                    }),
            );
        }
        // Permanent delete: behind a confirmation — it can't be undone.
        let w = w.clone();
        tree.add_child(
            row,
            Button::new("\u{2715}")
                .radius(4.0)
                .background(settings::error())
                .on_click(move |ctx| permanent_delete(&w, note_id, ctx)),
        );
    } else {
        // Move to trash. Reversible (restorable from the trash view), so unlike
        // the permanent delete it isn't gated behind a confirmation.
        let w = w.clone();
        tree.add_child(
            row,
            // Themed red so the destructive action stays legible on both light
            // and dark surfaces.
            Button::new("\u{2715}")
                .radius(4.0)
                .background(settings::error())
                .on_click(move |ctx| {
                    move_note_to_trash(&w, note_id);
                    // The row left the live list, and its tags may have been the
                    // last of their kind — rebuild the list + filter row (the
                    // latter prunes any now-orphaned filter tags).
                    rebuild_sidebar(&w, ctx);
                    // Selection may have moved to a sibling (or none) — sync the
                    // editor's chip row to whatever note is active now.
                    w.tag_refresh.fire(ctx);
                }),
        );
    }
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

/// Move a note to the trash — a reversible soft delete (`AppState::trash_note`
/// stamps `deleted_at` and re-selects a live sibling). If the trashed note was
/// the active one, the editor rebases onto whatever is selected now (or an
/// empty editor when the bin took the last live note). The caller rebuilds the
/// sidebar + fires the tag refresh.
fn move_note_to_trash(w: &SidebarWiring, note_id: NoteId) {
    let was_selected = matches!(
        &w.state.borrow().phase,
        Phase::Unlocked { selected, .. } if *selected == Some(note_id)
    );
    w.state.borrow_mut().trash_note(note_id);
    if was_selected {
        rebase_editor_to_selected(w);
    }
}

/// Permanently delete a trashed note, behind a confirmation — it can't be
/// undone. On confirm the row and its on-disk blob are dropped, the editor
/// rebases if the deleted note was showing, and the sidebar rebuilds.
fn permanent_delete(w: &SidebarWiring, note_id: NoteId, ctx: &mut EventContext) {
    let w = w.clone();
    confirm_dialog(
        ctx,
        Key::TrashDeleteConfirmTitle,
        Key::TrashDeleteConfirmBody,
        Key::TrashDeleteConfirmBtn,
        move |ctx| {
            let was_selected = matches!(
                &w.state.borrow().phase,
                Phase::Unlocked { selected, .. } if *selected == Some(note_id)
            );
            if let Err(e) = w.state.borrow_mut().permanent_delete_note(note_id) {
                notice::show(format!("{}{e}", i18n::tr(Key::ErrDeleteNotePrefix)));
                return;
            }
            if was_selected {
                rebase_editor_to_selected(&w);
            }
            rebuild_sidebar(&w, ctx);
            w.tag_refresh.fire(ctx);
        },
    );
}

/// Permanently delete every trashed note, behind a confirmation. Rebases the
/// editor (the selection may have been a trashed note) and rebuilds the
/// sidebar.
fn empty_trash(w: &SidebarWiring, ctx: &mut EventContext) {
    let w = w.clone();
    confirm_dialog(
        ctx,
        Key::TrashEmptyConfirmTitle,
        Key::TrashEmptyConfirmBody,
        Key::TrashEmptyConfirmBtn,
        move |ctx| {
            if let Err(e) = w.state.borrow_mut().empty_trash() {
                notice::show(format!("{}{e}", i18n::tr(Key::ErrEmptyTrashPrefix)));
                return;
            }
            rebase_editor_to_selected(&w);
            rebuild_sidebar(&w, ctx);
            w.tag_refresh.fire(ctx);
        },
    );
}

/// Rebase the editor's title / body signals onto the currently selected note,
/// or clear them to an empty editor when nothing is selected. Called after a
/// delete (trash / permanent / empty) that may have moved or cleared the
/// selection. Setting `body` also re-renders the live preview, so no stale
/// preview of a just-removed note can linger.
fn rebase_editor_to_selected(w: &SidebarWiring) {
    let payload = {
        let s = w.state.borrow();
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
        w.title.set(t);
        w.body.set(b);
    }
}

/// Push a modal confirmation: a title, a body, and Cancel / `<confirm>`
/// buttons. `on_confirm` runs when the user confirms, then the layer is
/// dismissed; Cancel just dismisses. Used for the two destructive trash
/// actions (permanent delete + empty trash), mirroring the backup-restore
/// confirmation idiom.
fn confirm_dialog(
    ctx: &mut EventContext,
    title: Key,
    body: Key,
    confirm: Key,
    on_confirm: impl Fn(&mut EventContext) + 'static,
) {
    ctx.push_layer(
        LayerOptions::modal(),
        Container::column()
            .width(420.0)
            .padding(24.0)
            .gap(16.0)
            .background(settings::surface())
            .radius(12.0),
        move |tree, dialog| {
            tree.add_child(
                dialog,
                TextWidget::reactive(move || i18n::tr(title).to_string())
                    .font_size(20.0)
                    .color(settings::on_surface()),
            );
            tree.add_child(
                dialog,
                TextWidget::reactive(move || i18n::tr(body).to_string())
                    .color(settings::on_surface_variant()),
            );
            let buttons = tree.add_child(dialog, Container::row().gap(8.0).justify_center());
            tree.add_child(
                buttons,
                Button::reactive_label(|| i18n::tr(Key::TrashCancel).to_string())
                    .radius(6.0)
                    .on_click(|ctx| ctx.pop_top_layer()),
            );
            tree.add_child(
                buttons,
                Button::reactive_label(move || i18n::tr(confirm).to_string())
                    .radius(6.0)
                    .background(settings::error())
                    .on_click(move |ctx| {
                        on_confirm(ctx);
                        ctx.pop_top_layer();
                    }),
            );
        },
    );
}
