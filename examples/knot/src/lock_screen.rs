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

use shroud::reactive::Reactive;
use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, SecureInput, TextWidget};

use crate::crypto::{derive_key, unwrap_dek};
use crate::i18n::{self, Key};
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
    tree.add_child(
        card,
        TextWidget::reactive(|| i18n::tr(Key::Tagline).to_string()),
    );

    tree.add_child(
        card,
        TextWidget::reactive(|| i18n::tr(Key::LockMasterPasswordLabel).to_string()),
    );

    let unlock_state = Rc::clone(&state);
    let input_idx = tree.add_child(
        card,
        SecureInput::new()
            .placeholder(i18n::tr(Key::LockPasswordPlaceholder))
            .on_submit(move |master, ctx| {
                if try_unlock(&unlock_state, master) {
                    let next = Rc::clone(&unlock_state);
                    ctx.replace_screen(move |tree| vault_screen::build(tree, next));
                }
            }),
    );
    tree.focus_initially(input_idx);

    // Status line. A live lockout countdown takes priority over the phase
    // error; both recompute every per-frame tick (which repaints), so the
    // countdown ticks down on its own without a dedicated timer.
    let status_state = Rc::clone(&state);
    let color_state = Rc::clone(&state);
    tree.add_child(
        card,
        TextWidget::reactive(move || {
            let s = status_state.borrow();
            if let Some(rem) = s.lockout_remaining() {
                // Round up so the final fractional second still reads "1s".
                let secs = rem.as_secs() + 1;
                return i18n::tr(Key::TooManyAttempts).replace("{n}", &secs.to_string());
            }
            match &s.phase {
                Phase::Locked { error: None } => i18n::tr(Key::Locked).to_string(),
                Phase::Locked { error: Some(e) } => {
                    format!("{}{}", i18n::tr(Key::LockedErrorPrefix), e)
                }
                // Setup / Recovery / Unlocked are unreachable here — on success
                // the handler queues a replace_screen, so by the next paint
                // we're already off this screen. Render empty defensively.
                Phase::Setup { .. } | Phase::Recovery { .. } | Phase::Unlocked { .. } => {
                    String::new()
                }
            }
        })
        .color(Reactive::derive(move || {
            let theme = settings::current_theme();
            let s = color_state.borrow();
            if s.lockout_remaining().is_some()
                || matches!(&s.phase, Phase::Locked { error: Some(_) })
            {
                theme.colors.error
            } else {
                theme.colors.on_surface_variant
            }
        })),
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
            Button::reactive_label(|| i18n::tr(Key::ForgotPassword).to_string())
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
    // Refuse attempts while a lockout is in effect. The status line's reactive
    // already shows the countdown, so there's nothing to set here — just don't
    // burn an (expensive) Argon2 derivation on a guess we won't honor.
    if state.borrow().lockout_remaining().is_some() {
        return false;
    }

    let Some(paths) = VaultPaths::default_for_app() else {
        set_error(state, i18n::tr(Key::ConfigUnavailable).to_string());
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
            set_error(
                state,
                format!("{}{}", i18n::tr(Key::ErrReadKeyFilePrefix), e),
            );
            return false;
        }
    };
    let Some(dek) = unwrap_dek(&pw_kek, &wrapped) else {
        // A failed unwrap is the canonical wrong-password signal. Count it; if
        // that crosses the lockout threshold the status reactive switches to a
        // countdown, otherwise echo the plain wrong-password message.
        if state.borrow_mut().note_failed_unlock().is_none() {
            set_error(state, i18n::tr(Key::LockWrongPassword).to_string());
        }
        return false;
    };

    // Open the DB with the DEK. SQLCipher should accept it (the DEK was
    // the key used to create the DB); a BadKey here means the dek.enc and
    // vault.db drifted out of sync (e.g. a partial restore).
    let storage = match VaultStorage::open(&paths.db, &dek) {
        Ok(s) => s,
        Err(StorageError::BadKey) => {
            set_error(state, i18n::tr(Key::LockKeyMismatch).to_string());
            return false;
        }
        Err(e) => {
            set_error(state, format!("{}{}", i18n::tr(Key::ErrVaultPrefix), e));
            return false;
        }
    };

    let encrypted = match storage.load_notes() {
        Ok(n) => n,
        Err(e) => {
            set_error(state, format!("{}{}", i18n::tr(Key::ErrReadNotesPrefix), e));
            return false;
        }
    };

    // Per-row XChaCha decrypt — should always succeed if SQLCipher
    // accepted the DEK (same key on both layers). A failure here means
    // the DB was tampered with outside our control.
    let Some(notes) = decrypt_all(&dek, &encrypted) else {
        set_error(state, i18n::tr(Key::VaultDataCorrupted).to_string());
        return false;
    };

    // Success clears the failed-attempt streak so a later wrong password
    // starts counting from zero again.
    state.borrow_mut().reset_unlock_attempts();
    state.borrow_mut().become_unlocked(dek, notes, storage);

    // Opening the vault is the auto-backup trigger: take one now if the
    // configured cadence says it's due. Runs once per unlock (not per frame),
    // and the just-opened files are current since nothing's been edited yet.
    crate::backup::maybe_auto_backup();
    true
}

fn set_error(state: &Rc<RefCell<AppState>>, msg: String) {
    state.borrow_mut().phase = Phase::Locked { error: Some(msg) };
}
