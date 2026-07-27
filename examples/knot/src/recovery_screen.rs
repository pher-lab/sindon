//! Recovery screen — forgot-password flow driven by the BIP39 key.
//!
//! The user pastes/types their 12-word recovery key and chooses a new
//! password. The mnemonic re-derives the recovery KEK (HKDF-SHA256), which
//! unwraps the DEK from `recovery.enc`; the DEK is then re-wrapped under
//! the new password (fresh salt + `dek.enc`) and the vault opens. The DB
//! itself is never re-encrypted because the DEK is unchanged — recovery
//! only swaps which password lock guards it.
//!
//! ## Field flow (mirrors the setup screen)
//!
//! The mnemonic lives in a visible multiline [`Input`] bound to a
//! `Signal<String>` so the user can read back the words they typed. The
//! two password fields use the same two-Enter stash pattern as setup: a
//! `SecureInput` only yields its secret through its own `on_submit`, so
//! the new-password field stashes a copy and advances focus, and the
//! confirm field compares with `SecureString`'s constant-time `PartialEq`
//! before attempting recovery.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sindon::reactive::{Reactive, Signal};
use sindon::security::SecureString;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, ClearTrigger, Container, Input, SecureInput, TextWidget};

use crate::crypto::{derive_key, random_salt, recovery, unwrap_dek, wrap_dek};
use crate::i18n::{self, Key};
use crate::lock_screen;
use crate::settings;
use crate::state::{AppState, Phase, decrypt_all};
use crate::storage::{VaultPaths, VaultStorage};
use crate::vault_screen;

/// Minimum new-password length. Same typo guard as setup.
const MIN_PASSWORD_LEN: usize = 8;

pub fn build(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    {
        let mut s = state.borrow_mut();
        if !matches!(s.phase, Phase::Recovery { .. }) {
            s.phase = Phase::Recovery { error: None };
        }
    }

    // The typed mnemonic. Visible (not secret-masked) so the user can
    // verify the words against what they wrote down.
    let mnemonic_sig = Signal::new(String::new());

    // Copy of the first new-password, held between the two Enter presses.
    let stash: Rc<RefCell<Option<SecureString>>> = Rc::new(RefCell::new(None));
    let confirm_idx: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let clear_pw = ClearTrigger::new();
    let clear_cf = ClearTrigger::new();

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

    tree.add_child(
        card,
        TextWidget::reactive(|| i18n::tr(Key::RecoveryTitle).to_string()).font_size(28.0),
    );
    tree.add_child(
        card,
        TextWidget::reactive(|| i18n::tr(Key::RecoveryDescription).to_string())
            .color(settings::on_surface_variant()),
    );

    // Recovery-key field (visible, multiline).
    tree.add_child(
        card,
        Input::new()
            .placeholder(i18n::tr(Key::RecoveryKeyPlaceholder))
            .multiline()
            .lines(3)
            .value(mnemonic_sig),
    );

    // New password — Enter validates length, stashes a copy, advances.
    let pw_state = Rc::clone(&state);
    let pw_stash = Rc::clone(&stash);
    let pw_confirm_idx = Rc::clone(&confirm_idx);
    let pw_idx = tree.add_child(
        card,
        SecureInput::new()
            .placeholder(i18n::tr(Key::NewPasswordPlaceholder))
            .clear_on(clear_pw)
            .on_submit(move |pw, ctx| {
                if pw.char_count() < MIN_PASSWORD_LEN {
                    set_error(
                        &pw_state,
                        i18n::tr(Key::ValidationMinLength)
                            .replace("{n}", &MIN_PASSWORD_LEN.to_string()),
                    );
                    return;
                }
                *pw_stash.borrow_mut() = Some(pw.expose(SecureString::new));
                pw_state.borrow_mut().phase = Phase::Recovery { error: None };
                ctx.focus(pw_confirm_idx.get());
            }),
    );

    // Confirm — Enter compares against the stash, then recovers on a match.
    let cf_state = Rc::clone(&state);
    let cf_stash = Rc::clone(&stash);
    let confirm = tree.add_child(
        card,
        SecureInput::new()
            .placeholder(i18n::tr(Key::RecoveryConfirmPlaceholder))
            .clear_on(clear_cf)
            .on_submit(move |confirm_pw, ctx| {
                let matched = match cf_stash.borrow().as_ref() {
                    Some(first) => first == confirm_pw,
                    None => {
                        set_error(&cf_state, i18n::tr(Key::ConfirmFirstRecovery).to_string());
                        ctx.focus(pw_idx);
                        return;
                    }
                };
                if !matched {
                    set_error(&cf_state, i18n::tr(Key::PasswordsMismatch).to_string());
                    cf_stash.borrow_mut().take();
                    clear_pw.bump();
                    clear_cf.bump();
                    ctx.focus(pw_idx);
                    return;
                }
                let phrase = mnemonic_sig.get_clone();
                match attempt_recovery(&cf_state, phrase.trim(), confirm_pw) {
                    Ok(()) => {
                        cf_stash.borrow_mut().take();
                        let next = Rc::clone(&cf_state);
                        ctx.replace_screen(move |tree| vault_screen::build(tree, next));
                    }
                    Err(e) => {
                        set_error(&cf_state, e);
                        // Keep the typed mnemonic so the user can fix a
                        // single mistyped word; only reset the passwords.
                        cf_stash.borrow_mut().take();
                        clear_pw.bump();
                        clear_cf.bump();
                        ctx.focus(pw_idx);
                    }
                }
            }),
    );
    confirm_idx.set(confirm);

    // Status line: errors in red, otherwise a neutral prompt.
    let status_state = Rc::clone(&state);
    let color_state = Rc::clone(&state);
    tree.add_child(
        card,
        TextWidget::reactive(move || match &status_state.borrow().phase {
            Phase::Recovery { error: Some(e) } => e.clone(),
            Phase::Recovery { error: None } => i18n::tr(Key::RecoveryPrompt).to_string(),
            _ => String::new(),
        })
        .color(Reactive::derive(move || {
            let theme = settings::current_theme();
            match &color_state.borrow().phase {
                Phase::Recovery { error: Some(_) } => theme.colors.error,
                _ => theme.colors.on_surface_variant,
            }
        })),
    );

    // Back to the unlock screen without recovering.
    let back_state = Rc::clone(&state);
    tree.add_child(
        card,
        Button::reactive_label(|| i18n::tr(Key::RecoveryBack).to_string())
            .radius(8.0)
            .on_click(move |ctx| {
                let next = Rc::clone(&back_state);
                ctx.replace_screen(move |tree| lock_screen::build(tree, next));
            }),
    );

    tree.focus_initially(pw_idx);
}

/// Re-derive the recovery KEK from `phrase`, unwrap the DEK from
/// `recovery.enc`, re-wrap it under `new_password` (fresh salt + `dek.enc`),
/// and open the vault into `Unlocked`. Returns a human-readable error on
/// any failure so the confirm handler can echo it on the status line.
fn attempt_recovery(
    state: &Rc<RefCell<AppState>>,
    phrase: &str,
    new_password: &SecureString,
) -> Result<(), String> {
    if phrase.split_whitespace().count() != recovery::WORD_COUNT {
        return Err(
            i18n::tr(Key::RecoveryWordCount).replace("{n}", &recovery::WORD_COUNT.to_string())
        );
    }

    let Some(paths) = VaultPaths::default_for_app() else {
        return Err(i18n::tr(Key::ConfigUnavailable).to_string());
    };

    let rec_kek = recovery::key_to_kek(phrase)
        .ok_or_else(|| i18n::tr(Key::RecoveryKeyInvalid).to_string())?;

    let wrapped = paths
        .read_wrapped_recovery()
        .map_err(|_| i18n::tr(Key::RecoveryNotSetUp).to_string())?;

    let dek = unwrap_dek(&rec_kek, &wrapped)
        .ok_or_else(|| i18n::tr(Key::RecoveryKeyInvalid).to_string())?;

    // Re-wrap the recovered DEK under the new password and install it.
    let new_salt = random_salt();
    let new_pw_kek = new_password.expose(|p| derive_key(p.as_bytes(), &new_salt));
    let new_wrapped = wrap_dek(&new_pw_kek, &dek);
    paths
        .write_salt(&new_salt)
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrWriteSaltPrefix), e))?;
    paths
        .write_wrapped_dek(&new_wrapped)
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrWriteKeyFilePrefix), e))?;

    // Open the DB with the recovered DEK and load the notes.
    let storage = VaultStorage::open(&paths.db, &dek)
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrOpenVaultPrefix), e))?;
    let encrypted = storage
        .load_notes()
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrReadNotesPrefix), e))?;
    let notes = decrypt_all(&dek, &encrypted)
        .ok_or_else(|| i18n::tr(Key::VaultDataCorrupted).to_string())?;

    let mut s = state.borrow_mut();
    s.salt = new_salt;
    s.become_unlocked(dek, notes, storage);
    Ok(())
}

fn set_error(state: &Rc<RefCell<AppState>>, msg: String) {
    state.borrow_mut().phase = Phase::Recovery { error: Some(msg) };
}
