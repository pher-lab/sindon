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
//!   * While unlocked: an auto-save tick (`on_frame`) flushes dirty
//!     notes every `tick_interval` (default 500 ms). Lock also runs a
//!     belt-and-suspenders full rewrite before dropping the key.

mod crypto;
mod editor;
mod lock_screen;
mod setup_screen;
mod sidebar;
mod state;
mod storage;
mod vault_screen;

use std::cell::RefCell;
use std::rc::Rc;

use shroud::app::App;
use shroud::widgets::tree::WidgetTree;

use crate::state::{AppState, Phase};
use crate::storage::VaultPaths;

fn main() {
    App::new()
        .title("Knot \u{2014} M3")
        .size(1080, 720)
        .capture_prevention(true)
        .run(|scope| {
            let state = init_state();

            // Auto-save tick — re-runs every `App::tick_interval`
            // (default 500 ms) for as long as a frame hook is set.
            // Cheap when nothing's dirty (early-returns inside
            // flush_dirty), so leaving it always-on costs nothing. It's
            // a no-op in the Setup / Locked phases.
            let state_for_tick = Rc::clone(&state);
            scope.on_frame(move || {
                if let Err(e) = state_for_tick.borrow_mut().flush_dirty() {
                    eprintln!("knot: auto-save tick failed: {}", e);
                }
            });

            let mut tree = WidgetTree::new();
            build_initial_screen(&mut tree, state);
            tree
        });
}

/// Build the screen that matches the initial phase. `init_state` only
/// ever produces `Setup` (first launch) or `Locked` (existing vault);
/// the `Unlocked` arm is here for exhaustiveness and would only fire if
/// a future init path handed us an already-decrypted vault.
fn build_initial_screen(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    let phase_kind = match &state.borrow().phase {
        Phase::Setup { .. } => Screen::Setup,
        Phase::Locked { .. } => Screen::Lock,
        Phase::Unlocked { .. } => Screen::Vault,
    };
    match phase_kind {
        Screen::Setup => setup_screen::build(tree, state),
        Screen::Lock => lock_screen::build(tree, state),
        Screen::Vault => vault_screen::build(tree, state),
    }
}

enum Screen {
    Setup,
    Lock,
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
