//! Knot M3 — SQLCipher-backed multi-note editor with first-launch setup.
//!
//! Notes live in a SQLCipher 4 database at `<config>/knot/vault.db`,
//! page-encrypted under the Argon2-derived master key. Each note row also
//! carries an XChaCha20-Poly1305 ciphertext (the original M1 crypto), so
//! the two layers cover each other.
//!
//! Lifecycle:
//!   * First launch (no `vault.db` yet): the setup screen has the user
//!     choose a master password, then creates an empty vault and drops
//!     them into the editor.
//!   * Subsequent launch: lock screen prompts for the master password,
//!     opens the SQLCipher DB on success.
//!   * While unlocked: a per-frame tick (`on_frame`) flushes dirty notes
//!     to SQLCipher every `tick_interval` (default 500 ms) and enforces
//!     the configured auto-lock — after enough idle time with no input it
//!     re-locks the vault (drops the key, returns to the lock screen).
//!     Lock also runs a belt-and-suspenders full rewrite before dropping
//!     the key.

mod backlinks;
mod backup;
mod change_password;
mod crypto;
mod editor;
mod find_replace;
mod highlight;
mod i18n;
mod icons;
mod lock_screen;
mod notice;
mod preview;
mod recovery_pdf;
mod recovery_screen;
mod settings;
mod setup_screen;
mod sidebar;
mod smart_keymap;
mod state;
mod storage;
mod tag_editor;
mod toolbar;
mod tooltip;
mod vault_screen;

use std::cell::RefCell;
use std::rc::Rc;

use shroud::app::App;
use shroud::reactive::Reactive;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Key, Modifiers, Shortcut};

use crate::state::{AppState, Phase};
use crate::storage::VaultPaths;

fn main() {
    // Establish the settings + OS-theme signals before the event loop
    // starts. This loads the persisted theme/font from disk so they apply
    // from the first frame, and ensures the initial OS-theme snapshot
    // taken in `resumed()` lands on the same handle `current_theme()`
    // reads (rather than a second one created lazily after startup).
    settings::signals();
    shroud::app::system_theme_signal();

    App::new()
        .title("Knot \u{2014} M3")
        .size(1080, 720)
        // Register the bundled icon font (FW-12) before the first paint, so the
        // editor toolbar can draw its formatting glyphs. See `icons`.
        .font(icons::FONT)
        .capture_prevention(true)
        // Theme is derived live from the settings signals + OS appearance,
        // re-evaluated every paint — a settings change repaints the whole
        // app without a tree rebuild (see `settings::current_theme`).
        .theme(Reactive::derive(settings::current_theme))
        .run(|scope| {
            let state = init_state();

            // Per-frame tick — re-runs every `App::tick_interval`
            // (default 500 ms) for as long as a frame hook is set. Does
            // two jobs, both no-ops outside `Unlocked`:
            //
            //   1. Auto-save: flush dirty notes to SQLCipher. Cheap when
            //      nothing's dirty (early-returns inside `flush_dirty`),
            //      so leaving it always-on costs nothing.
            //   2. Auto-lock: when the configured idle timeout has elapsed
            //      with no user input, re-encrypt + drop the key and return
            //      to the lock screen — the same flow as the manual Lock
            //      button, driven by inactivity instead of a click.
            let state_for_tick = Rc::clone(&state);
            scope.on_frame(move |ctx| {
                // Hover-tooltip poll (FW-13): shows a tip once the cursor has
                // rested on a trigger past its delay. Cheap when idle.
                tooltip::tick(ctx);

                if let Err(e) = state_for_tick.borrow_mut().flush_dirty() {
                    // Surface to the banner (it also logs) so the user knows
                    // their last edits haven't reached disk — not just stderr.
                    notice::show(format!(
                        "{}{e}",
                        crate::i18n::tr(crate::i18n::Key::ErrSaveChangesPrefix)
                    ));
                }

                let is_unlocked = matches!(state_for_tick.borrow().phase, Phase::Unlocked { .. });
                if is_unlocked {
                    if let Some(timeout) = settings::current_auto_lock().timeout() {
                        if ctx.idle() >= timeout {
                            state_for_tick.borrow_mut().lock_and_seal();
                            let next = Rc::clone(&state_for_tick);
                            ctx.event_ctx
                                .replace_screen(move |tree| lock_screen::build(tree, next));
                        }
                    }
                }
            });

            // Ctrl+F focuses the sidebar search box. Global scope so it fires
            // even while the editor (an `Input`) has focus — the usual
            // "find from anywhere" feel. The box's tree index is recorded on
            // the app state by `sidebar::build`; we only act while unlocked,
            // and `focus` drops silently if the recorded index is stale after
            // a screen rebuild.
            let search_state = Rc::clone(&state);
            scope.on_shortcut(
                Shortcut::global(Modifiers::CTRL, Key::Character('f')),
                move |ctx| {
                    let target = {
                        let s = search_state.borrow();
                        match s.phase {
                            Phase::Unlocked { .. } => s.search_input_idx,
                            _ => None,
                        }
                    };
                    if let Some(idx) = target {
                        ctx.event_ctx.focus(idx);
                    }
                },
            );

            // Ctrl+H toggles the editor's find-replace bar. Global so it fires
            // from anywhere (including while the body Input has focus — the
            // Input leaves Ctrl+letter combos other than its own undo/redo for
            // the shortcut router). Opening focuses the Find field; closing
            // returns focus to the body. Both node indices are recorded on the
            // app state by the editor build; we only act while unlocked with a
            // note selected (the bar lives in the editor area, hidden otherwise).
            let find_state = Rc::clone(&state);
            scope.on_shortcut(
                Shortcut::global(Modifiers::CTRL, Key::Character('h')),
                move |ctx| {
                    let (find_idx, body_idx) = {
                        let s = find_state.borrow();
                        match &s.phase {
                            Phase::Unlocked {
                                selected: Some(_), ..
                            } => (s.find_input_idx, s.body_input_idx),
                            _ => (None, None),
                        }
                    };
                    // No editor in scope (locked / no note selected) → ignore.
                    let Some(find_idx) = find_idx else {
                        return;
                    };
                    let sigs = find_replace::signals();
                    let now_visible = !sigs.visible.get();
                    sigs.visible.set(now_visible);
                    if now_visible {
                        ctx.event_ctx.focus(find_idx);
                    } else if let Some(body_idx) = body_idx {
                        ctx.event_ctx.focus(body_idx);
                    }
                },
            );

            let mut tree = WidgetTree::new();
            build_initial_screen(&mut tree, state);
            tree
        });
}

/// Build the screen that matches the initial phase. `init_state` only
/// ever produces `Setup` (first launch) or `Locked` (existing vault);
/// the `Recovery` / `Unlocked` arms are here for exhaustiveness — recovery
/// is only ever reached mid-session via `replace_screen` from the lock
/// screen, and `Unlocked` would only fire if a future init path handed us
/// an already-decrypted vault.
fn build_initial_screen(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    let phase_kind = match &state.borrow().phase {
        Phase::Setup { .. } => Screen::Setup,
        Phase::Locked { .. } => Screen::Lock,
        Phase::Recovery { .. } => Screen::Recovery,
        Phase::Unlocked { .. } => Screen::Vault,
    };
    match phase_kind {
        Screen::Setup => setup_screen::build(tree, state),
        Screen::Lock => lock_screen::build(tree, state),
        Screen::Recovery => recovery_screen::build(tree, state),
        Screen::Vault => vault_screen::build(tree, state),
    }
}

enum Screen {
    Setup,
    Lock,
    Recovery,
    Vault,
}

/// Resolve the on-disk vault state into an `AppState`.
///
/// First launch (no `vault.db`) returns a `Setup` state so the user can
/// choose their own master password. Subsequent launches read the salt
/// file and return a `Locked` state for the lock screen to handle.
/// Persistence misconfiguration (no OS config dir, unreadable salt) is
/// surfaced via `panic!` rather than silently falling back to in-memory
/// mode — Knot's whole point is durable storage, and pretending
/// otherwise hides a real problem from the user.
fn init_state() -> Rc<RefCell<AppState>> {
    let paths = VaultPaths::default_for_app()
        .expect("knot: OS config directory unavailable — cannot run without persistence");

    if paths.vault_exists() {
        let salt = paths
            .read_salt()
            .unwrap_or_else(|e| panic!("knot: failed to read vault.salt: {}", e));
        Rc::new(RefCell::new(AppState::new_locked(salt, 1)))
    } else {
        Rc::new(RefCell::new(AppState::new_setup()))
    }
}
