//! Tag editor (app-side composite).
//!
//! A row of removable pill "chips" plus a text input, with an inline
//! autocomplete list drawn from every tag already used in the vault. The
//! framework has no `TagEditor` widget and no reactive-children primitive,
//! so this is assembled from `Container` (flex-wrap chips), `Input`
//! (commit on Enter / comma), and `Button` (chip ✕ + suggestion rows),
//! and rebuilt imperatively via [`EventContext::rebuild_children`] whenever
//! the tag set or the input changes.
//!
//! The active note's tags live in [`AppState`] (the single source of truth);
//! this module never caches them. Each mutation re-reads `selected_tags()`
//! and rebuilds the chip row, so a tag added here and a note switched in the
//! sidebar both converge on the same render path.
//!
//! Switching notes happens in the sidebar, which has no handle on this
//! subtree — so [`TagRefresh`] bridges the two: `build` installs a rebuild
//! closure into it, and the sidebar `fire`s it after `select` / `create` /
//! `delete` so the chips track the newly selected note.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shroud::core::Color;
use shroud::reactive::{Reactive, Signal};
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, EventContext, Input, TextWidget};

use crate::settings;
use crate::sidebar::SidebarRefresh;
use crate::state::{AppState, normalize_tag};

/// Cap on how many autocomplete rows show at once — keeps the inline list
/// from pushing the body editor down when a vault has many tags.
const MAX_SUGGESTIONS: usize = 6;

// ── Theme-reactive chip colors (re-read per paint, follow a live theme swap,
//    mirroring the helpers in `preview.rs`). ─────────────────────────────────

fn chip_bg() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().colors.surface_variant)
}
fn chip_fg() -> Reactive<Color> {
    Reactive::derive(|| settings::current_theme().colors.primary)
}

/// The rebuild closure [`TagRefresh`] stores: re-renders the chips/suggestions
/// for whatever note is currently selected.
type RefreshFn = Rc<dyn Fn(&mut EventContext)>;

/// Shared handle that rebuilds the chip row + suggestion list (and clears the
/// input) from any event handler. The tag editor installs the real rebuild
/// closure during [`build`]; the sidebar fires it whenever the active note
/// changes, since it can't reach this subtree directly and there is no
/// reactive-children primitive to do it declaratively.
#[derive(Clone, Default)]
pub struct TagRefresh {
    inner: Rc<RefCell<Option<RefreshFn>>>,
}

impl TagRefresh {
    pub fn new() -> Self {
        Self::default()
    }

    fn install(&self, f: RefreshFn) {
        *self.inner.borrow_mut() = Some(f);
    }

    /// Rebuild the chips/suggestions for the currently selected note. No-op
    /// until [`build`] has installed the closure (i.e. before the editor pane
    /// exists). Clones the closure out before calling it so the `RefCell`
    /// borrow isn't held across the rebuild.
    pub fn fire(&self, ctx: &mut EventContext) {
        let f = self.inner.borrow().clone();
        if let Some(f) = f {
            f(ctx);
        }
    }
}

/// The shared bits every rebuild helper needs: the app state, the input's
/// bound signal (so commits can clear it), and the two container indices to
/// rebuild into. Cheap to clone — `Signal` is `Copy` and the rest is `Rc`.
#[derive(Clone)]
struct Wiring {
    state: Rc<RefCell<AppState>>,
    input: Signal<String>,
    chips: Rc<Cell<usize>>,
    suggestions: Rc<Cell<usize>>,
    /// Editor → sidebar bridge, fired after a tag is added or removed so the
    /// sidebar's filter chips track the vault's (possibly grown / shrunk) tag
    /// set. The sidebar installs the rebuild; here we only fire it.
    sidebar_refresh: SidebarRefresh,
}

/// Build the tag editor under `parent` and install the cross-module refresh
/// closure into `refresh`. Lays out a wrapping chip row, the tag input, and
/// an (initially empty) inline suggestion column.
pub fn build(
    tree: &mut WidgetTree,
    parent: usize,
    state: Rc<RefCell<AppState>>,
    refresh: &TagRefresh,
    sidebar_refresh: SidebarRefresh,
) {
    let w = Wiring {
        state,
        input: Signal::new(String::new()),
        chips: Rc::new(Cell::new(0)),
        suggestions: Rc::new(Cell::new(0)),
        sidebar_refresh,
    };

    let section = tree.add_child(parent, Container::column().width_full().gap(6.0));

    // Chips wrap onto multiple lines when a note has many tags. Holds only
    // the chips; the input below is a separate, persistent widget so adding
    // a tag (which rebuilds this row) never steals focus mid-typing.
    let chips_row = tree.add_child(
        section,
        Container::row()
            .width_full()
            .flex_wrap(true)
            .gap(6.0)
            .align_center(),
    );
    w.chips.set(chips_row);

    let input_sig = w.input;
    let w_submit = w.clone();
    let w_change = w.clone();
    tree.add_child(
        section,
        Input::new()
            .placeholder("Add tags\u{2026}")
            .value(input_sig)
            .font_size(13.0)
            // Enter commits the input (preferring the top autocomplete match,
            // mirroring the reference app).
            .on_submit(move |raw, ctx| submit_from_input(&w_submit, raw, ctx))
            // Typing refreshes suggestions; a comma commits the segment(s)
            // before it (so "a,b," adds two tags and leaves the trailing
            // partial in the field).
            .on_change(move |val, ctx| on_input_change(&w_change, val, ctx)),
    );

    let suggestions = tree.add_child(section, Container::column().width_full().gap(2.0));
    w.suggestions.set(suggestions);

    populate_chips(tree, chips_row, &w);

    let w_refresh = w.clone();
    refresh.install(Rc::new(move |ctx: &mut EventContext| {
        // A note switch clears any in-progress tag entry and re-renders both
        // lists against the newly selected note.
        w_refresh.input.set(String::new());
        rebuild_chips(&w_refresh, ctx);
        rebuild_suggestions(&w_refresh, ctx);
    }));
}

/// Re-render the chip row from the selected note's current tags.
fn rebuild_chips(w: &Wiring, ctx: &mut EventContext) {
    let parent = w.chips.get();
    let w = w.clone();
    ctx.rebuild_children(parent, move |tree, parent| populate_chips(tree, parent, &w));
}

/// Re-render the inline suggestion list for the current input buffer.
fn rebuild_suggestions(w: &Wiring, ctx: &mut EventContext) {
    let parent = w.suggestions.get();
    let w = w.clone();
    ctx.rebuild_children(parent, move |tree, parent| {
        populate_suggestions(tree, parent, &w)
    });
}

/// One pill per tag: the label plus a ✕ that removes it.
fn populate_chips(tree: &mut WidgetTree, parent: usize, w: &Wiring) {
    let tags = w.state.borrow().selected_tags();
    for tag in tags {
        let chip = tree.add_child(
            parent,
            Container::row()
                .align_center()
                .gap(4.0)
                .padding(4.0)
                .radius(10.0)
                .background(chip_bg()),
        );
        tree.add_child(
            chip,
            TextWidget::new(tag.clone())
                .color(chip_fg())
                .font_size(13.0),
        );

        let w = w.clone();
        let to_remove = tag.clone();
        tree.add_child(
            chip,
            Button::new("\u{2715}")
                .font_size(12.0)
                .background(Color::TRANSPARENT)
                .hover_background(settings::hover())
                .text_color(settings::on_surface_variant())
                .radius(8.0)
                .on_click(move |ctx| {
                    w.state.borrow_mut().remove_tag_from_selected(&to_remove);
                    rebuild_chips(&w, ctx);
                    // Removing a tag can free it up to reappear as a suggestion.
                    rebuild_suggestions(&w, ctx);
                    // It may also have been the vault's last use of that tag —
                    // refresh the sidebar filter chips (which prunes it).
                    w.sidebar_refresh.fire(ctx);
                }),
        );
    }
}

/// Inline autocomplete: vault tags that contain the (normalized) query and
/// aren't already on this note. Empty query or no matches renders nothing.
fn populate_suggestions(tree: &mut WidgetTree, parent: usize, w: &Wiring) {
    let query = normalize_tag(&w.input.get_clone());
    if query.is_empty() {
        return;
    }
    let (current, all) = {
        let s = w.state.borrow();
        (s.selected_tags(), s.all_tags())
    };
    let matches: Vec<String> = all
        .into_iter()
        .filter(|t| t.contains(&query) && !current.iter().any(|c| c == t))
        .take(MAX_SUGGESTIONS)
        .collect();
    if matches.is_empty() {
        return;
    }

    let list = tree.add_child(
        parent,
        Container::column()
            .width(220.0)
            .padding(4.0)
            .gap(2.0)
            .radius(8.0)
            .background(settings::surface()),
    );
    for suggestion in matches {
        let w = w.clone();
        let value = suggestion.clone();
        tree.add_child(
            list,
            Button::new(suggestion)
                .font_size(13.0)
                .background(Color::TRANSPARENT)
                .hover_background(settings::hover())
                .text_color(settings::on_surface())
                .radius(4.0)
                .grow(1.0)
                .on_click(move |ctx| commit_tag(&w, &value, ctx)),
        );
    }
}

/// Add `raw` to the selected note (normalized + deduped by `AppState`), clear
/// the input, and rebuild. Always rebuilds suggestions (the cleared input
/// collapses the list); rebuilds chips only when a tag was really added.
fn commit_tag(w: &Wiring, raw: &str, ctx: &mut EventContext) {
    let added = w.state.borrow_mut().add_tag_to_selected(raw);
    w.input.set(String::new());
    if added {
        rebuild_chips(w, ctx);
        // A genuinely new tag may have grown the vault's tag set — refresh the
        // sidebar filter chips so it shows up there too.
        w.sidebar_refresh.fire(ctx);
    }
    rebuild_suggestions(w, ctx);
}

/// Enter handler: commit the top autocomplete match for the current input,
/// or the raw text when nothing matches (mirrors the reference app's Enter
/// behavior so a partial type + Enter snaps to an existing tag).
fn submit_from_input(w: &Wiring, raw: &str, ctx: &mut EventContext) {
    let query = normalize_tag(raw);
    if query.is_empty() {
        return;
    }
    let chosen = {
        let s = w.state.borrow();
        let current = s.selected_tags();
        s.all_tags()
            .into_iter()
            .find(|t| t.contains(&query) && !current.iter().any(|c| c == t))
            .unwrap_or(query)
    };
    commit_tag(w, &chosen, ctx);
}

/// on_change handler: a comma commits every complete segment before it and
/// keeps the trailing partial; otherwise it just refreshes the suggestions.
fn on_input_change(w: &Wiring, val: &str, ctx: &mut EventContext) {
    if val.contains(',') {
        let mut segments: Vec<&str> = val.split(',').collect();
        // The text after the last comma is still being typed — keep it.
        let remainder = segments.pop().unwrap_or("").to_string();
        let mut added_any = false;
        for seg in segments {
            if w.state.borrow_mut().add_tag_to_selected(seg) {
                added_any = true;
            }
        }
        w.input.set(remainder);
        if added_any {
            rebuild_chips(w, ctx);
            // New tags may have grown the vault's tag set — refresh the
            // sidebar filter chips.
            w.sidebar_refresh.fire(ctx);
        }
        rebuild_suggestions(w, ctx);
    } else {
        rebuild_suggestions(w, ctx);
    }
}
