//! Change-master-password modal — re-wrap the DEK under a new password.
//!
//! Opened from the settings modal while the vault is unlocked. The user
//! supplies their current password (authorization — so a passer-by can't
//! silently change the password on an unattended unlocked session), then a
//! new password and a confirmation.
//!
//! ## Why the notes are never re-encrypted
//!
//! Knot keys the DB and every note with a random **DEK** that is wrapped on
//! disk under a password-derived KEK (`dek.enc`) and a recovery KEK
//! (`recovery.enc`). Changing the password only re-wraps the *same* DEK under
//! a KEK derived from the new password — the database and `recovery.enc` are
//! left untouched, so the existing recovery key keeps working and not a single
//! note row is rewritten. (This mirrors the recovery flow, which also rewraps
//! the DEK rather than re-encrypting the vault.)
//!
//! The Argon2 **salt is reused**, not rotated. A salt is per-vault (it exists
//! to make the KDF output unique to this vault and defeat precomputation), not
//! per-password, so reusing it across a password change is sound — and it
//! means only one file (`dek.enc`) changes. That matters for durability: salt
//! and `dek.enc` must always agree (unlock derives the KEK from the salt to
//! unwrap `dek.enc`), and rotating both would open a two-file window where a
//! crash between writes leaves them inconsistent. Rewrapping `dek.enc` alone,
//! written atomically, has no such window.
//!
//! ## Field flow (mirrors setup / recovery)
//!
//! A `SecureInput` only surfaces its secret through its own `on_submit`, so
//! confirmation is field-to-field: Enter in the current-password field stashes
//! a copy and advances; Enter in the new-password field validates length,
//! stashes, and advances; Enter in the confirm field compares (constant-time
//! `SecureString` `PartialEq`) and, on a match, performs the change. Stashed
//! copies live in `SecureString` (zeroized on drop) and are taken the moment
//! they're no longer needed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shroud::reactive::{Reactive, Signal};
use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, ClearTrigger, Container, SecureInput, TextWidget};

use crate::crypto::{derive_key, unwrap_dek, wrap_dek};
use crate::i18n::{self, Key};
use crate::settings;
use crate::state::AppState;
use crate::storage::VaultPaths;

/// Minimum new-password length. Same typo guard as setup / recovery — the
/// real safety net for a forgotten password is the BIP39 recovery key, not a
/// strength policy.
const MIN_PASSWORD_LEN: usize = 8;

/// Status of the modal's single feedback line, driving both its text color and
/// the dismiss button's label.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    /// No attempt yet (or a field was advanced): show the neutral prompt.
    Idle,
    /// The last attempt failed: show the message in the error color.
    Error,
    /// The password was changed: show the message in the success color.
    Success,
}

/// Populate the change-password dialog body. Used as the `push_layer`
/// populate closure, so the only context it needs is the `WidgetTree`, the
/// dialog's root index, and the app state (to read the salt and re-wrap the
/// DEK). The dismiss button pops the layer through its own `EventContext`.
pub fn populate(tree: &mut WidgetTree, dialog: usize, state: Rc<RefCell<AppState>>) {
    // Feedback line state. We're `Unlocked`, so unlike setup / recovery there's
    // no phase error slot to borrow — the modal owns these signals directly.
    let msg = Signal::new(String::new());
    let status = Signal::new(Status::Idle);

    // Copies of the current / new passwords, held between Enter presses.
    let cur_stash: Rc<RefCell<Option<SecureString>>> = Rc::new(RefCell::new(None));
    let new_stash: Rc<RefCell<Option<SecureString>>> = Rc::new(RefCell::new(None));

    // Forward references to fields that don't exist yet when an earlier field's
    // handler is built (same Cell trick as setup's `confirm_idx`).
    let new_idx: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let confirm_idx: Rc<Cell<usize>> = Rc::new(Cell::new(0));

    // Reset a field to empty on a mismatch / failure so the user retypes from
    // scratch rather than backspacing through stale masked characters.
    let clear_cur = ClearTrigger::new();
    let clear_new = ClearTrigger::new();
    let clear_cf = ClearTrigger::new();

    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::ChangePasswordTitle).to_string())
            .font_size(22.0)
            .color(settings::on_surface()),
    );
    tree.add_child(
        dialog,
        TextWidget::reactive(|| i18n::tr(Key::ChangePasswordDescription).to_string())
            .color(settings::on_surface_variant()),
    );

    // Current password — Enter stashes a copy and advances. No verification
    // here; the confirm handler verifies it (by trying to unwrap the DEK) so a
    // single failure path reports "current password is incorrect".
    let cur_stash_h = Rc::clone(&cur_stash);
    let cur_new_idx = Rc::clone(&new_idx);
    let cur_idx = tree.add_child(
        dialog,
        SecureInput::new()
            .placeholder(i18n::tr(Key::ChangePasswordCurrentPlaceholder))
            .clear_on(clear_cur)
            .on_submit(move |cur, ctx| {
                *cur_stash_h.borrow_mut() = Some(cur.expose(SecureString::new));
                msg.set(String::new());
                status.set(Status::Idle);
                ctx.focus(cur_new_idx.get());
            }),
    );

    // New password — Enter validates length, stashes, and advances.
    let new_stash_h = Rc::clone(&new_stash);
    let new_confirm_idx = Rc::clone(&confirm_idx);
    let new_field_idx = tree.add_child(
        dialog,
        SecureInput::new()
            .placeholder(i18n::tr(Key::NewPasswordPlaceholder))
            .clear_on(clear_new)
            .on_submit(move |np, ctx| {
                if np.char_count() < MIN_PASSWORD_LEN {
                    msg.set(
                        i18n::tr(Key::ValidationMinLength)
                            .replace("{n}", &MIN_PASSWORD_LEN.to_string()),
                    );
                    status.set(Status::Error);
                    return;
                }
                *new_stash_h.borrow_mut() = Some(np.expose(SecureString::new));
                msg.set(String::new());
                status.set(Status::Idle);
                ctx.focus(new_confirm_idx.get());
            }),
    );
    new_idx.set(new_field_idx);

    // Confirm — Enter compares against the new-password stash, then performs
    // the change using the current-password stash to authorize it.
    let cf_state = Rc::clone(&state);
    let cf_cur_stash = Rc::clone(&cur_stash);
    let cf_new_stash = Rc::clone(&new_stash);
    let confirm = tree.add_child(
        dialog,
        SecureInput::new()
            .placeholder(i18n::tr(Key::RecoveryConfirmPlaceholder))
            .clear_on(clear_cf)
            .on_submit(move |confirm_pw, ctx| {
                // The confirm must match the stashed new password.
                let matched = match cf_new_stash.borrow().as_ref() {
                    Some(first) => first == confirm_pw,
                    None => {
                        // Confirm submitted before the new-password field.
                        msg.set(i18n::tr(Key::ConfirmFirstRecovery).to_string());
                        status.set(Status::Error);
                        ctx.focus(new_field_idx);
                        return;
                    }
                };
                if !matched {
                    msg.set(i18n::tr(Key::PasswordsMismatch).to_string());
                    status.set(Status::Error);
                    cf_new_stash.borrow_mut().take();
                    clear_new.bump();
                    clear_cf.bump();
                    ctx.focus(new_field_idx);
                    return;
                }

                // Take the current-password copy out of the stash (so it's
                // zeroized when this handler returns regardless of outcome).
                let current = cf_cur_stash.borrow_mut().take();
                let Some(current) = current else {
                    msg.set(i18n::tr(Key::ChangePasswordEnterCurrentFirst).to_string());
                    status.set(Status::Error);
                    ctx.focus(cur_idx);
                    return;
                };

                match attempt_change(&cf_state, &current, confirm_pw) {
                    Ok(()) => {
                        cf_new_stash.borrow_mut().take();
                        clear_cur.bump();
                        clear_new.bump();
                        clear_cf.bump();
                        // Keep the modal open showing the success line; the
                        // dismiss button (now labeled "Done") closes it.
                        msg.set(i18n::tr(Key::ChangePasswordSuccess).to_string());
                        status.set(Status::Success);
                    }
                    Err(e) => {
                        msg.set(e);
                        status.set(Status::Error);
                        cf_new_stash.borrow_mut().take();
                        clear_cur.bump();
                        clear_new.bump();
                        clear_cf.bump();
                        ctx.focus(cur_idx);
                    }
                }
            }),
    );
    confirm_idx.set(confirm);

    // Feedback line: the neutral prompt until something is set, then the most
    // recent error / success message in the matching theme color.
    tree.add_child(
        dialog,
        TextWidget::reactive(move || {
            let m = msg.get_clone();
            if m.is_empty() {
                i18n::tr(Key::ChangePasswordPrompt).to_string()
            } else {
                m
            }
        })
        .color(Reactive::derive(move || {
            let theme = settings::current_theme();
            match status.get() {
                Status::Error => theme.colors.error,
                Status::Success => theme.colors.success,
                Status::Idle => theme.colors.on_surface_variant,
            }
        })),
    );

    // Dismiss. Reads as "Cancel" until the change lands, then "Done" so a
    // post-success click doesn't read as discarding anything.
    let done_row = tree.add_child(dialog, Container::row().gap(8.0).justify_center());
    tree.add_child(
        done_row,
        Button::reactive_label(move || {
            if status.get() == Status::Success {
                i18n::tr(Key::SettingsDone).to_string()
            } else {
                i18n::tr(Key::ChangePasswordCancel).to_string()
            }
        })
        .radius(6.0)
        .on_click(|ctx| ctx.pop_top_layer()),
    );

    // Land focus on the current-password field so the user can type straight
    // away (applied on the next redraw, after the push-layer command drains).
    tree.focus_initially(cur_idx);
}

/// Verify `current` against the on-disk `dek.enc`, then re-wrap the recovered
/// DEK under `new` (same salt) and overwrite `dek.enc` atomically. Returns a
/// human-readable error for the feedback line on any failure. Does **not**
/// touch the database, `recovery.enc`, or the salt — see the module docs.
fn attempt_change(
    state: &Rc<RefCell<AppState>>,
    current: &SecureString,
    new: &SecureString,
) -> Result<(), String> {
    let Some(paths) = VaultPaths::default_for_app() else {
        return Err(i18n::tr(Key::ConfigUnavailable).to_string());
    };

    // The salt is stable across a password change, so the in-memory copy and
    // the on-disk salt agree; reading from state avoids a disk round-trip.
    let salt = state.borrow().salt;

    // Authorize: the current password must unwrap the DEK from dek.enc. A wrong
    // password is an AEAD auth failure here (unwrap → None).
    let cur_kek = current.expose(|p| derive_key(p.as_bytes(), &salt));
    let wrapped = paths
        .read_wrapped_dek()
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrReadKeyFilePrefix), e))?;
    let dek = unwrap_dek(&cur_kek, &wrapped)
        .ok_or_else(|| i18n::tr(Key::ChangePasswordCurrentWrong).to_string())?;

    // Re-wrap the same DEK under the new password (same salt) and replace
    // dek.enc atomically so a crash can't truncate it.
    let new_kek = new.expose(|p| derive_key(p.as_bytes(), &salt));
    let new_wrapped = wrap_dek(&new_kek, &dek);
    paths
        .write_wrapped_dek_atomic(&new_wrapped)
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrWriteKeyFilePrefix), e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::crypto::{derive_key, generate_dek, random_salt, unwrap_dek, wrap_dek};

    /// The crypto invariant behind `attempt_change`: re-wrapping the DEK under
    /// a new password (reusing the salt) leaves the DEK unchanged, makes the
    /// old password stop unwrapping it, and makes the new password unwrap it.
    #[test]
    fn rewrap_under_new_password_keeps_dek_and_rotates_the_lock() {
        let dek = generate_dek();
        let salt = random_salt();

        let old_kek = derive_key(b"old-password", &salt);
        let old_wrapped = wrap_dek(&old_kek, &dek);
        assert_eq!(
            unwrap_dek(&old_kek, &old_wrapped).unwrap().as_ref(),
            dek.as_ref(),
            "the old password unwraps the original wrapping"
        );

        // Re-wrap under the new password with the *same* salt (what
        // `attempt_change` does on disk).
        let new_kek = derive_key(b"new-password", &salt);
        let new_wrapped = wrap_dek(&new_kek, &dek);

        // The old password no longer opens the new wrapping...
        assert!(
            unwrap_dek(&old_kek, &new_wrapped).is_none(),
            "the old password must stop working after the change"
        );
        // ...the new password does, and recovers the identical DEK, so the DB
        // (keyed by that DEK) needs no re-encryption.
        assert_eq!(
            unwrap_dek(&new_kek, &new_wrapped).unwrap().as_ref(),
            dek.as_ref(),
            "the new password unwraps the same DEK"
        );
    }
}
