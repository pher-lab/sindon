//! Vault screen — the post-unlock app shell.
//!
//! Layout is a column at the root: a dismissable error banner across the top
//! (collapsed to nothing when idle — see [`crate::notice`]), then a content
//! row with the sidebar pane (fixed width) on the left and the editor pane
//! (flex-grow) on the right. The Lock button lives in the editor's header so
//! it's adjacent to the content the user just decrypted, matching the Knot
//! v0.7.0 layout.
//!
//! The two `Signal<String>` (title + body) live for the lifetime of this
//! screen tree; switching notes calls `signal.set(...)` to rebase the
//! editor's bound `Input`s on the next paint.

use std::cell::RefCell;
use std::rc::Rc;

use sindon::reactive::{Reactive, Signal};
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, TextWidget};

use crate::editor;
use crate::notice;
use crate::settings;
use crate::sidebar;
use crate::sidebar::SidebarRefresh;
use crate::state::{AppState, Phase};
use crate::tag_editor::TagRefresh;

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

    // Clear any banner left over from a previous unlocked session so a stale
    // error doesn't greet the user on re-entry.
    notice::dismiss();

    // Root is a column: a dismissable error banner across the top (collapsed to
    // zero height when there's nothing to show), then the main content row
    // (sidebar | editor) that fills the rest.
    //
    // `overflow_hidden` on the content row is load-bearing, not cosmetic: the
    // row grows to fill the shell's height, but a flex item's automatic minimum
    // size is its content — so a tall note list or a long preview would balloon
    // the row past the window, every pane (the sidebar's `height_full` column,
    // the editor) would stretch to that, and the `grow` ScrollViews inside would
    // have nothing to scroll (no scrollbar, the wheel dead, and the sidebar's
    // settings button shoved off the bottom). Pinning the row's automatic
    // minimum to 0 makes it clamp to the allocated height so the panes size it
    // and the content scrolls inside — the same trick the ScrollView and the
    // editor's inner split row already use on themselves.
    let shell = tree.set_root(Container::column().width_full().height_full());
    build_error_banner(tree, shell);
    let root = tree.add_child(
        shell,
        Container::row().width_full().grow(1.0).overflow_hidden(),
    );

    // Bridges the sidebar (which switches the active note) to the editor's
    // tag chips (which must re-render for the newly selected note). The
    // editor installs the rebuild closure; the sidebar fires it on every
    // select / create / delete.
    let tag_refresh = TagRefresh::new();

    // The reverse bridge: the editor fires this after a tag is added or
    // removed so the sidebar's tag-filter chips track the vault's tag set.
    // The sidebar installs the rebuild closure.
    let sidebar_refresh = SidebarRefresh::new();

    sidebar::build(
        tree,
        root,
        Rc::clone(&state),
        title_sig,
        body_sig,
        tag_refresh.clone(),
        sidebar_refresh.clone(),
    );

    editor::build(
        tree,
        root,
        state,
        title_sig,
        body_sig,
        preview_sig,
        tag_refresh,
        sidebar_refresh,
    );
}

/// Top-of-window banner for user-facing failures (see [`crate::notice`]).
/// Visible only while a message is set, so it collapses to nothing the rest of
/// the time; the message text reads live from the notice signal and the ✕
/// dismisses it. Styled on the theme error color so it reads as a warning on
/// both light and dark.
fn build_error_banner(tree: &mut WidgetTree, parent: usize) {
    let banner = tree.add_child(
        parent,
        Container::row()
            .width_full()
            .padding(10.0)
            .gap(12.0)
            .align_center()
            .background(settings::error())
            .visible(Reactive::derive(|| notice::signal().get_clone().is_some())),
    );

    tree.add_child(
        banner,
        TextWidget::reactive(|| notice::signal().get_clone().unwrap_or_default())
            // on_primary is the theme's high-contrast text color; it reads
            // cleanly on the saturated error fill on both light and dark.
            .color(Reactive::derive(|| {
                settings::current_theme().colors.on_primary
            })),
    );

    // Spacer pushes the dismiss button to the right edge.
    tree.add_child(banner, Container::row().grow(1.0));

    tree.add_child(
        banner,
        Button::new("\u{2715}")
            .radius(4.0)
            .background(sindon::core::Color::TRANSPARENT)
            .text_color(Reactive::derive(|| {
                settings::current_theme().colors.on_primary
            }))
            .on_click(|_ctx| notice::dismiss()),
    );
}
