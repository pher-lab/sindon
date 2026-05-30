//! Lock screen — centered card with the master-password field.
//!
//! On submit, derives the password KEK, unwraps the DEK from `dek.enc`,
//! and opens the SQLCipher vault with the DEK. A wrong password fails to
//! unwrap the DEK (AEAD auth failure) — that's the primary wrong-password
//! signal now, ahead of SQLCipher. The per-row XChaCha20-Poly1305 layer
//! is a third line of defence and any auth failure there indicates a
//! tampered DB rather than a wrong password.
//!
//! A "Forgot password?" link drops into the recovery screen, but only
//! when a `recovery.enc` wrapping exists on disk.

use std::cell::RefCell;
use std::rc::Rc;

use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, SecureInput, TextWidget};

use crate::crypto::{derive_key, unwrap_dek};
use crate::recovery_screen;
use crate::settings;
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
            .justify_center(),
    );

    let card = tree.add_child(
        root,
        Container::column()
            .width(448.0)
            .margin_x_auto()
            .padding(32.0)
            .gap(16.0)
            .background(settings::surface())
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
            // Setup / Recovery / Unlocked are unreachable here — on success
            // the handler queues a replace_screen, so by the next paint
            // we're already off this screen. Render empty defensively.
            Phase::Setup { .. } | Phase::Recovery { .. } | Phase::Unlocked { .. } => String::new(),
        })
        .color(settings::on_surface_variant()),
    );

    // Offer recovery only when a recovery wrapping was created at setup.
    // A vault with no recovery.enc can't be recovered, so hide the path
    // rather than dangle a button that always errors.
    let has_recovery = VaultPaths::default_for_app()
        .map(|p| p.recovery_exists())
        .unwrap_or(false);
    if has_recovery {
        let recover_state = Rc::clone(&state);
        tree.add_child(
            card,
            Button::new("Forgot password? Use your recovery key")
                .radius(8.0)
                .on_click(move |ctx| {
                    let next = Rc::clone(&recover_state);
                    ctx.replace_screen(move |tree| recovery_screen::build(tree, next));
                }),
        );
    }
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
    let pw_kek = master.expose(|m| derive_key(m.as_bytes(), &salt));

    // Unwrap the DEK from dek.enc with the password KEK. A wrong password
    // produces an AEAD auth failure here — the primary wrong-password
    // signal, before SQLCipher is ever touched.
    let wrapped = match paths.read_wrapped_dek() {
        Ok(w) => w,
        Err(e) => {
            set_error(state, format!("failed to read key file: {}", e));
            return false;
        }
    };
    let Some(dek) = unwrap_dek(&pw_kek, &wrapped) else {
        set_error(state, "wrong master password".into());
        return false;
    };

    // Open the DB with the DEK. SQLCipher should accept it (the DEK was
    // the key used to create the DB); a BadKey here means the dek.enc and
    // vault.db drifted out of sync (e.g. a partial restore).
    let storage = match VaultStorage::open(&paths.db, &dek) {
        Ok(s) => s,
        Err(StorageError::BadKey) => {
            set_error(state, "vault key mismatch (corrupted or partial restore)".into());
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
    // accepted the DEK (same key on both layers). A failure here means
    // the DB was tampered with outside our control.
    let Some(notes) = decrypt_all(&dek, &encrypted) else {
        set_error(state, "vault data corrupted (auth failed)".into());
        return false;
    };

    state.borrow_mut().become_unlocked(dek, notes, storage);
    true
}

fn set_error(state: &Rc<RefCell<AppState>>, msg: String) {
    state.borrow_mut().phase = Phase::Locked { error: Some(msg) };
}
