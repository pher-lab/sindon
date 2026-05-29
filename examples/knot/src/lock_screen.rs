//! Lock screen — centered card with the master-password field.
//!
//! On submit, opens the SQLCipher vault with the derived key. SQLCipher
//! itself rejects a wrong password (we surface that as
//! [`StorageError::BadKey`]); the per-row XChaCha20-Poly1305 layer is a
//! second line of defence and any auth failure there indicates a
//! tampered DB rather than a wrong password.

use std::cell::RefCell;
use std::rc::Rc;

use shroud::core::Color;
use shroud::reactive::Reactive;
use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, SecureInput, TextWidget};

use crate::crypto::derive_key;
use crate::state::{AppState, Phase, decrypt_all};
use crate::storage::{StorageError, VaultPaths, VaultStorage};
use crate::vault_screen;

pub fn build(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    // Reset transient phase: keep the previous error message if there was one
    // (so a wrong-password attempt is still visible after the bounce), but
    // drop any decrypted notes / key that might be leftover.
    {
        let mut s = state.borrow_mut();
        if !matches!(s.phase, Phase::Locked { .. }) {
            s.phase = Phase::Locked { error: None };
        }
    }

    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .padding(24.0)
            .justify_center()
            .align_center(),
    );

    let card = tree.add_child(
        root,
        Container::column()
            .width_full()
            .max_width(448.0)
            .padding(32.0)
            .gap(16.0)
            .background(Color::rgb(0.12, 0.12, 0.18))
            .radius(16.0),
    );

    tree.add_child(card, TextWidget::new("Knot").font_size(40.0));
    tree.add_child(card, TextWidget::new("A knot only you can untie."));

    tree.add_child(card, TextWidget::new("Master password:"));

    let unlock_state = Rc::clone(&state);
    let input_idx = tree.add_child(
        card,
        SecureInput::new()
            .placeholder("Enter master password, press Enter to unlock")
            .on_submit(move |master, ctx| {
                if try_unlock(&unlock_state, master) {
                    let next = Rc::clone(&unlock_state);
                    ctx.replace_screen(move |tree| vault_screen::build(tree, next));
                }
            }),
    );
    tree.focus_initially(input_idx);

    let status_state = Rc::clone(&state);
    tree.add_child(
        card,
        TextWidget::reactive(move || match &status_state.borrow().phase {
            Phase::Locked { error: None } => "Locked.".to_string(),
            Phase::Locked { error: Some(e) } => format!("Locked \u{2014} {}", e),
            // Setup / Unlocked are unreachable here — on success the
            // handler queues a replace_screen, so by the next paint we're
            // already off this screen. Render an empty string defensively.
            Phase::Setup { .. } | Phase::Unlocked { .. } => String::new(),
        })
        .color(Reactive::Static(Color::rgb(0.7, 0.7, 0.75))),
    );
}

/// Try to unlock the vault with `master`. Returns `true` on success.
/// On any failure path, writes a human-readable error into
/// `Phase::Locked.error` so the status text updates on the next paint
/// and the user knows what to try next.
fn try_unlock(state: &Rc<RefCell<AppState>>, master: &SecureString) -> bool {
    let Some(paths) = VaultPaths::default_for_app() else {
        set_error(state, "config directory unavailable".into());
        return false;
    };

    let salt = state.borrow().salt;
    let key = master.expose(|m| derive_key(m.as_bytes(), &salt));

    // SQLCipher refuses to open with the wrong key — that's the
    // primary wrong-password signal. A non-BadKey error means
    // something more serious (disk gone, file truncated, etc.) and
    // gets surfaced verbatim.
    let storage = match VaultStorage::open(&paths.db, &key) {
        Ok(s) => s,
        Err(StorageError::BadKey) => {
            set_error(state, "wrong master password".into());
            return false;
        }
        Err(e) => {
            set_error(state, format!("vault error: {}", e));
            return false;
        }
    };

    let encrypted = match storage.load_notes() {
        Ok(n) => n,
        Err(e) => {
            set_error(state, format!("failed to read notes: {}", e));
            return false;
        }
    };

    // Per-row XChaCha decrypt — should always succeed if SQLCipher
    // accepted the key (same key on both layers). A failure here
    // means the DB was tampered with outside our control.
    let Some(notes) = decrypt_all(&key, &encrypted) else {
        set_error(state, "vault data corrupted (auth failed)".into());
        return false;
    };

    state.borrow_mut().become_unlocked(key, notes, storage);
    true
}

fn set_error(state: &Rc<RefCell<AppState>>, msg: String) {
    state.borrow_mut().phase = Phase::Locked { error: Some(msg) };
}
