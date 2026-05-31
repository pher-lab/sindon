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

mod crypto;
mod editor;
mod lock_screen;
mod preview;
mod recovery_screen;
mod settings;
mod setup_screen;
mod sidebar;
mod state;
mod storage;
mod vault_screen;

use std::cell::RefCell;
use std::rc::Rc;

use shroud::app::App;
use shroud::reactive::Reactive;
use shroud::widgets::tree::WidgetTree;

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
                if let Err(e) = state_for_tick.borrow_mut().flush_dirty() {
                    eprintln!("knot: auto-save tick failed: {}", e);
                }

                let is_unlocked =
                    matches!(state_for_tick.borrow().phase, Phase::Unlocked { .. });
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
