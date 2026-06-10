//! Editor pane — title input + multiline body, plus the Lock button.
//!
//! Both inputs are bound to `Signal<String>` so that switching notes
//! (via the sidebar) only needs `signal.set(...)` — the Input widget
//! rebases its buffer from the signal on the next paint without us having
//! to rebuild the subtree. `on_change` writes back to the selected note
//! in `AppState`.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc;

use shroud::platform::FileDialog;
use shroud::reactive::{Reactive, Signal};
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, Image, Input, ReactiveChildren, ScrollView, TextWidget};

use crate::backlinks;
use crate::i18n::{self, Key};
use crate::lock_screen;
use crate::notice;
use crate::preview;
use crate::settings;
use crate::sidebar::SidebarRefresh;
use crate::state::{AppState, Phase};
use crate::tag_editor::{self, TagRefresh};
use crate::toolbar;

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

// Wide by nature: the editor pane threads the two shared note signals, the
// edit/preview toggle, and both refresh bridges alongside the usual
// tree/parent/state. They have no smaller natural grouping at this single call
// site (from `vault_screen`), so a bundling struct would be churn for no gain.
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

    // Accept image files dragged from the OS file manager onto the window.
    // winit carries no drop coordinates (see `WidgetTree::on_file_drop`), so
    // this is a window-level hook rather than a drop-zone: a dropped image is
    // inserted into the *currently selected* note exactly as the "Image"
    // button does. Non-image files, and drops while no note is selected, are
    // ignored. Registered per vault screen so it's cleared automatically when
    // the vault locks — a drop on the lock screen does nothing.
    let drop_state = Rc::clone(&state);
    tree.on_file_drop(move |path, _ctx| {
        if !note_selected(&drop_state) || !is_image_path(path) {
            return;
        }
        insert_image_from_path(&drop_state, body_sig, path);
    });

    // Accept an image pasted from the clipboard (Ctrl/Cmd+V with image
    // content — e.g. a screenshot, or an image copied from a browser). The
    // framework hands over already-encoded PNG bytes, so this shares the
    // same store-and-append tail as the drop handler and the Image button.
    // Pastes while no note is selected are ignored. Like the drop handler,
    // it's registered per vault screen and cleared when the vault locks.
    let paste_state = Rc::clone(&state);
    tree.on_image_paste(move |png, _ctx| {
        if !note_selected(&paste_state) {
            return;
        }
        insert_image_from_bytes(&paste_state, body_sig, png);
    });

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
                            i18n::tr(Key::Untitled)
                        } else {
                            note.title.as_str()
                        };
                        i18n::tr(Key::EditorEditing).replace("{title}", title)
                    } else {
                        i18n::tr(Key::EditorNoNoteSelected).to_string()
                    }
                } else {
                    i18n::tr(Key::EditorNoNoteSelectedHint).to_string()
                }
            }
            _ => String::new(),
        })
        .color(settings::on_surface_variant()),
    );

    // Spacer pushes the trailing buttons to the far right.
    tree.add_child(header, Container::row().grow(1.0));

    // Insert-image button. Picks a PNG/JPEG, stores it as an encrypted
    // attachment, and appends a `![image](knot-img:<id>)` reference to the
    // body. Shown whenever a note is selected — in the split layout the editor
    // stays present alongside the preview, so inserting raw markdown always
    // makes sense. The bytes are decrypted again only when the preview renders
    // the reference (see `preview::emit_image_block`).
    let img_btn_state = Rc::clone(&state);
    let img_vis_state = Rc::clone(&state);
    tree.add_child(
        header,
        Button::reactive_label(|| i18n::tr(Key::EditorImageBtn).to_string())
            .radius(8.0)
            .visible(Reactive::derive(move || note_selected(&img_vis_state)))
            .on_click(move |_ctx| {
                let Some(path) = FileDialog::new()
                    .title(i18n::tr(Key::DialogInsertImage))
                    .filter("Images", &["png", "jpg", "jpeg"])
                    .open_file()
                else {
                    return;
                };
                insert_image_from_path(&img_btn_state, body_sig, &path);
            }),
    );

    // Export button. Writes the selected note's body verbatim to a chosen
    // `<title>.md`. Tags are never written, so an export can't leak the
    // vault's encrypted tag metadata to a plaintext file. Visible only when a
    // note is selected, like the Image button.
    let export_state = Rc::clone(&state);
    let export_vis_state = Rc::clone(&state);
    tree.add_child(
        header,
        Button::reactive_label(|| i18n::tr(Key::EditorExportBtn).to_string())
            .radius(8.0)
            .visible(Reactive::derive(move || note_selected(&export_vis_state)))
            .on_click(move |_ctx| export_selected(&export_state)),
    );

    // Preview toggle. Shows / hides the live preview pane (built below in the
    // content row). The pane renders the body live through `ReactiveChildren`,
    // so the toggle only flips the flag — no manual rebuild. Hidden when no
    // note is selected so the header's "No note selected" prompt stands alone.
    let toggle_visible_state = Rc::clone(&state);
    tree.add_child(
        header,
        Button::reactive_label(move || {
            if preview_sig.get() {
                i18n::tr(Key::EditorPreviewHide).to_string()
            } else {
                i18n::tr(Key::EditorPreviewShow).to_string()
            }
        })
        .radius(8.0)
        .visible(Reactive::derive(move || {
            note_selected(&toggle_visible_state)
        }))
        .on_click(move |_ctx| {
            preview_sig.set(!preview_sig.get());
        }),
    );

    let lock_state = Rc::clone(&state);
    tree.add_child(
        header,
        Button::reactive_label(|| i18n::tr(Key::EditorLockBtn).to_string())
            .radius(8.0)
            .on_click(move |ctx| {
                // Re-encrypt the current notes into the vault, then transition
                // to the lock screen. `lock_and_seal` drops the key + plaintext
                // notes; Zeroizing ensures the key is wiped.
                lock_state.borrow_mut().lock_and_seal();
                let next = Rc::clone(&lock_state);
                ctx.replace_screen(move |tree| lock_screen::build(tree, next));
            }),
    );

    // Content row: editor pane on the left, live preview pane on the right.
    // The preview collapses (`display: none`) when toggled off, letting the
    // editor take the full width; when on, the two share the row.
    //
    // `overflow_hidden` is essential, not cosmetic: this row grows to fill the
    // pane's leftover height, but a flex item's automatic minimum size is its
    // content — so the row would otherwise balloon to the (tall) preview's
    // content height, every child would stretch to that, and the preview's
    // ScrollView would have nothing to scroll. `overflow_hidden` pins the row's
    // automatic minimum to 0 so it clamps to the allocated height instead, the
    // same trick the ScrollView uses on itself.
    let content_row = tree.add_child(
        pane,
        Container::row()
            .width_full()
            .grow(1.0)
            .gap(16.0)
            .overflow_hidden(),
    );

    // Editor area: title + body inputs. Hidden via `display: none` when no
    // note is selected, so the header's "No note selected" prompt stands alone
    // instead of showing inputs that look editable but silently drop typing
    // (`write_selected` no-ops without a selection). Unlike before, it stays
    // visible while previewing — the preview now sits beside it, not over it.
    //
    // `flex_basis(0)` (CSS `flex: 1 1 0`) pins each pane to the row's leftover
    // width split rather than its content's natural width, so a long preview
    // heading can't squeeze the editor (mirrors the pane's own basis above).
    let area_state = Rc::clone(&state);
    let editor_area = tree.add_child(
        content_row,
        Container::column()
            .grow(1.0)
            .flex_basis(0.0)
            .gap(12.0)
            .visible(Reactive::derive(move || note_selected(&area_state))),
    );

    // Title input (single-line, full width).
    let title_state = Rc::clone(&state);
    tree.add_child(
        editor_area,
        Input::new()
            .placeholder(i18n::tr(Key::EditorTitlePlaceholder))
            .value(title_sig)
            .font_size(20.0)
            .on_change(move |new_title, _ctx| {
                write_selected(&title_state, |note| {
                    note.title = new_title.to_string();
                });
            }),
    );

    // Tag editor (chips + input + inline autocomplete), between the title and
    // the body so it stays visible without scrolling. It reads/writes the
    // selected note's tags directly; `tag_refresh` lets the sidebar rebuild
    // the chips when the active note changes, and `sidebar_refresh` lets the
    // tag editor rebuild the sidebar's filter chips when a tag is added or
    // removed here.
    tag_editor::build(
        tree,
        editor_area,
        Rc::clone(&state),
        &tag_refresh,
        sidebar_refresh,
    );

    // Formatting toolbar (Heading / Bold / Italic / … ), directly above the
    // body so its inserts visibly land in the editor below. It reads/writes the
    // body through `body_sig` + `cursor_sig` and re-focuses the body via
    // `body_idx`, which we fill in once the body input is built (just below).
    let cursor_sig = Signal::new(0usize);
    let body_idx = Rc::new(Cell::new(0usize));
    toolbar::build(
        tree,
        editor_area,
        Rc::clone(&state),
        body_sig,
        cursor_sig,
        Rc::clone(&body_idx),
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
    let body_node = tree.add_child(
        body_wrap,
        Input::new()
            .placeholder(i18n::tr(Key::EditorBodyPlaceholder))
            .multiline()
            .lines(16)
            .value(body_sig)
            // Mirror the caret so the toolbar can insert at it (see `toolbar`).
            .cursor_signal(cursor_sig)
            .on_change(move |new_body, _ctx| {
                write_selected(&body_state, |note| {
                    note.body = new_body.to_string();
                });
            }),
    );
    body_idx.set(body_node);

    // Preview area: a scrollable, live-rendered markdown view beside the
    // editor. Visible only while a note is selected *and* the preview is
    // toggled on. The `ReactiveChildren` inside re-renders the body whenever it
    // changes (keyed on `body_token`), so edits in the left pane appear here on
    // the next frame without any manual rebuild.
    let preview_state = Rc::clone(&state);
    let preview_area = tree.add_child(
        content_row,
        Container::column()
            .grow(1.0)
            .flex_basis(0.0)
            // The ScrollView fills this wrapper's height via `grow`, but a flex
            // item defaults to a content-sized minimum — so without this the
            // wrapper would balloon to the (overflowing) preview content's
            // height instead of clamping to the pane's leftover space, and a
            // tall image would leave nothing to scroll. `overflow_hidden` lets
            // the pane size it and the content scroll inside (the ScrollView
            // sets the same on itself; the wrapper between them needs it too).
            .overflow_hidden()
            .visible(Reactive::derive(move || {
                note_selected(&preview_state) && preview_sig.get()
            })),
    );
    let preview_scroll = tree.add_child(preview_area, ScrollView::new().width_full().grow(1.0));

    // Navigation handle for `[[wikilink]]` clicks inside the preview: clicking
    // one selects the matching note (the live preview then re-renders for the
    // new body). Owned by the builder below, which re-uses it on every rebuild.
    let nav = preview::WikiNav::new(Rc::clone(&state), title_sig, body_sig, tag_refresh.clone());
    tree.add_child(
        preview_scroll,
        ReactiveChildren::column().width_full().gap(12.0).source(
            move || preview_token(preview_sig.get(), &body_sig.get_clone()),
            move |tree, parent| {
                let body = body_sig.get_clone();
                preview::render(tree, parent, &body, Some(&nav));
            },
        ),
    );

    // Backlinks panel: notes that link to the selected one via `[[wikilink]]`.
    // Lives at the bottom of the pane (below the edit/preview row) so it's
    // visible in both edit and preview modes. Clicking a backlink navigates to
    // that note exactly like a wikilink click. Sits in `pane` after
    // `content_row`, so it's the editor pane's last child.
    backlinks::build(tree, pane, state, title_sig, body_sig, tag_refresh);
}

/// Change token for the live preview. Combines the toggle state with a hash of
/// the body, so `ReactiveChildren` rebuilds the preview exactly when it needs
/// to:
///
/// * **Hidden** (`preview_on == false`): the body is *not* hashed, so the token
///   is constant no matter what the user types. This matters because the
///   default state is preview-off — without it, every keystroke would reparse
///   the markdown and rebuild an off-screen subtree.
/// * **Shown**: the token tracks the body, so each edit rebuilds the preview and
///   an unchanged body leaves it alone. Toggling on flips the token once,
///   rebuilding with the current body.
///
/// Hashing keeps it always correct with nothing to keep in sync (no separate
/// revision signal a new body-mutating path could forget to bump).
pub(crate) fn preview_token(preview_on: bool, body: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    preview_on.hash(&mut hasher);
    if preview_on {
        body.hash(&mut hasher);
    }
    hasher.finish()
}

/// True when `path` looks like a supported image by extension (PNG / JPEG,
/// case-insensitive). A cheap pre-filter for the drop handler so a dropped
/// `.txt` or `.zip` is ignored without reading it — the authoritative check
/// is still the decode in [`insert_image_from_path`].
fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.eq_ignore_ascii_case("png")
                || e.eq_ignore_ascii_case("jpg")
                || e.eq_ignore_ascii_case("jpeg")
        })
        .unwrap_or(false)
}

/// Read an image file and insert it into the selected note via
/// [`insert_image_from_bytes`]. Used by the "Image" button (file dialog)
/// and the drag-and-drop handler, both of which carry a path. No-op (with a
/// stderr note) when the file can't be read.
fn insert_image_from_path(state: &Rc<RefCell<AppState>>, body_sig: Signal<String>, path: &Path) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            notice::show(format!("{}{e}", i18n::tr(Key::ErrReadImagePrefix)));
            return;
        }
    };
    insert_image_from_bytes(state, body_sig, &bytes);
}

/// Store encoded image bytes as an encrypted attachment and append a
/// `![image](knot-img:<id>)` reference to the body. The shared core of the
/// "Image" button, the drag-and-drop handler (both via
/// [`insert_image_from_path`]), and the clipboard-paste handler (which gets
/// PNG bytes from the framework directly). No-op (with a stderr note) when
/// the bytes don't decode as a supported image or can't be stored. The
/// caller is responsible for only invoking this while a note is selected
/// (`write_selected` no-ops otherwise).
fn insert_image_from_bytes(state: &Rc<RefCell<AppState>>, body_sig: Signal<String>, bytes: &[u8]) {
    // Reject anything that doesn't decode *before* storing it, so a bad blob
    // never becomes a dead `knot-img:` reference.
    if Image::from_bytes(bytes).is_err() {
        notice::show(i18n::tr(Key::ErrImageUnsupported));
        return;
    }
    let Some(id) = state.borrow_mut().add_attachment(bytes) else {
        notice::show(i18n::tr(Key::ErrImageStore));
        return;
    };
    // Append the reference on its own line, keeping the bound signal and the
    // note body in lockstep. The body input is a text source, which always
    // rebases from the signal on the next paint, so the new line shows
    // immediately even when the body still holds focus (e.g. after a drop).
    let mut body = body_sig.get_clone();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&format!("![image]({}{})\n", crate::state::IMG_SCHEME, id));
    body_sig.set(body.clone());
    write_selected(state, move |note| note.body = body);
}

/// Apply `f` to the currently selected note's mutable state and mark
/// it dirty so the auto-save tick (`AppState::flush_dirty`) writes the
/// change back to SQLCipher within a tick interval. No-op when no
/// note is selected or the app is not in `Unlocked`.
pub(crate) fn write_selected<F>(state: &Rc<RefCell<AppState>>, f: F)
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

/// Export the selected note's body verbatim to a user-chosen `<title>.md`.
/// The title is carried only by the file name; tags and other metadata are
/// never written, so a plaintext export can't leak the vault's encrypted tag
/// metadata. No-op when locked, when nothing is selected, or when the user
/// cancels the save dialog. (Any embedded `knot-img:<id>` references are
/// written as-is — they round-trip back into Knot via Import but won't resolve
/// in other markdown viewers.) This is the inverse of `sidebar::import_note`.
fn export_selected(state: &Rc<RefCell<AppState>>) {
    let note = {
        let s = state.borrow();
        match &s.phase {
            Phase::Unlocked {
                notes,
                selected: Some(sel),
                ..
            } => notes
                .iter()
                .find(|n| n.id == *sel)
                .map(|n| (n.title.clone(), n.body.clone())),
            _ => None,
        }
    };
    let Some((title, body)) = note else {
        return;
    };

    let Some(path) = FileDialog::new()
        .title(i18n::tr(Key::DialogExportNote))
        .filter("Markdown", &["md"])
        .file_name(format!("{}.md", sanitize_filename(&title)))
        .save_file()
    else {
        return;
    };
    if let Err(e) = std::fs::write(&path, body) {
        notice::show(format!("{}{e}", i18n::tr(Key::ErrExportPrefix)));
    }
}

/// Turn a note title into a safe file-name stem: replace characters illegal in
/// file names on common OSes (and any control chars) with `_`, trim
/// surrounding whitespace and dots (Windows rejects trailing dots), and fall
/// back to `untitled` when nothing usable remains (e.g. an empty title).
fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_image_path_accepts_supported_extensions_case_insensitively() {
        assert!(is_image_path(Path::new("a.png")));
        assert!(is_image_path(Path::new("a.jpg")));
        assert!(is_image_path(Path::new("a.jpeg")));
        // Drag-and-drop hands over whatever case the file has on disk.
        assert!(is_image_path(Path::new("PHOTO.JPG")));
        assert!(is_image_path(Path::new("/some/dir/Shot.PNG")));
    }

    #[test]
    fn is_image_path_rejects_non_images_and_extensionless() {
        // The drop handler ignores these silently (no read, no insert).
        assert!(!is_image_path(Path::new("notes.txt")));
        assert!(!is_image_path(Path::new("archive.zip")));
        assert!(!is_image_path(Path::new("image.gif"))); // not a supported decode
        assert!(!is_image_path(Path::new("README")));
    }

    #[test]
    fn sanitize_filename_strips_illegal_chars_and_falls_back() {
        // Ordinary titles pass through untouched.
        assert_eq!(sanitize_filename("Shopping List"), "Shopping List");
        // Path separators and other reserved chars become underscores
        // (/ : * ? " < > | \ between c and d = seven illegal chars).
        assert_eq!(sanitize_filename("a/b:c*?\"<>|\\d"), "a_b_c_______d");
        // Surrounding whitespace and trailing dots (illegal on Windows) go.
        assert_eq!(sanitize_filename("  notes...  "), "notes");
        // Empty / whitespace-only → a stable fallback so the dialog still opens.
        assert_eq!(sanitize_filename(""), "untitled");
        assert_eq!(sanitize_filename("   "), "untitled");
        // An all-illegal title sanitizes to underscores — non-empty and a
        // perfectly valid file name, so it's kept as-is rather than masked.
        assert_eq!(sanitize_filename("/:*?"), "____");
        // Non-ASCII titles are preserved (CJK file names are fine on modern OSes).
        assert_eq!(sanitize_filename("買い物リスト"), "買い物リスト");
    }

    #[test]
    fn preview_token_freezes_while_hidden_and_tracks_body_when_shown() {
        // Hidden: the body is ignored, so the token is constant no matter what
        // the user types — no off-screen rebuilds while preview is off.
        assert_eq!(
            preview_token(false, "a"),
            preview_token(false, "b"),
            "while hidden the token must ignore body edits"
        );
        // Toggling on flips the token (rebuild once to show the current body).
        assert_ne!(
            preview_token(false, "a"),
            preview_token(true, "a"),
            "showing the preview must change the token"
        );
        // Shown: identical bodies are stable; an edit changes the token.
        assert_eq!(
            preview_token(true, "x\ny"),
            preview_token(true, "x\ny"),
            "same body while shown → same token"
        );
        assert_ne!(
            preview_token(true, "x\ny"),
            preview_token(true, "x\nz"),
            "an edit while shown changes the token"
        );
    }
}
