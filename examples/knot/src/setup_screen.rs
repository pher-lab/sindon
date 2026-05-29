//! Setup screen — first-launch master password creation.
//!
//! Shown once, when no vault exists on disk. Collects a master password
//! and a confirmation, then generates the salt, derives the key, and
//! creates an empty SQLCipher vault before dropping the user into the
//! editor.
//!
//! ## Why two Enter presses instead of a submit button
//!
//! A `SecureInput` only surfaces its secret to the framework through its
//! own `on_submit` handler — there is deliberately no way for a separate
//! button to read another field's `SecureString` (that would widen the
//! exposure of a secret beyond the field that owns it). So confirmation
//! works field-to-field: pressing Enter in the password field stashes a
//! copy of the secret and advances focus; pressing Enter in the confirm
//! field compares the two with `SecureString`'s constant-time `PartialEq`.
//! The stashed copy lives in a `SecureString` (zeroized on drop) and is
//! taken/cleared the moment it's no longer needed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shroud::core::Color;
use shroud::reactive::Reactive;
use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{ClearTrigger, Container, SecureInput, TextWidget};

use crate::crypto::{derive_key, random_salt};
use crate::state::{AppState, Phase};
use crate::storage::{VaultPaths, VaultStorage};
use crate::vault_screen;

/// Minimum master-password length (characters). There is no recovery
/// flow yet (BIP39 is a later milestone), so the floor is a typo guard,
/// not a strength policy.
const MIN_PASSWORD_LEN: usize = 8;

pub fn build(tree: &mut WidgetTree, state: Rc<RefCell<AppState>>) {
    {
        let mut s = state.borrow_mut();
        if !matches!(s.phase, Phase::Setup { .. }) {
            s.phase = Phase::Setup { error: None };
        }
    }

    // Copy of the first password, held between the two Enter presses so
    // the confirm field can compare against it. Zeroized when this
    // screen's tree drops (after replace_screen) or when taken below.
    let stash: Rc<RefCell<Option<SecureString>>> = Rc::new(RefCell::new(None));
    // The confirm field's tree index isn't known until it's inserted, so
    // the password handler reads it through a shared cell (same pattern
    // as the sidebar's list-parent cell).
    let confirm_idx: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    // Reset both fields on a mismatch so the user retypes from scratch
    // rather than backspacing through stale masked characters. `Copy`,
    // so the bound copy and the bumped copy share one counter.
    let clear_pw = ClearTrigger::new();
    let clear_cf = ClearTrigger::new();

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
    tree.add_child(
        card,
        TextWidget::new("Create a master password for your vault."),
    );

    // Two short lines instead of one long sentence: a single wide
    // TextWidget wraps at the card width and the wrapped line overlaps
    // the field below it, so keep each hint line under the wrap point.
    let hint_color = Color::rgb(0.7, 0.7, 0.75);
    let hint = tree.add_child(card, Container::column().gap(4.0));
    tree.add_child(
        hint,
        TextWidget::new(format!("At least {} characters.", MIN_PASSWORD_LEN)).color(hint_color),
    );
    tree.add_child(
        hint,
        TextWidget::new("No recovery yet \u{2014} don't forget it.").color(hint_color),
    );

    // Password field — Enter validates length, stashes a copy, advances.
    let pw_state = Rc::clone(&state);
    let pw_stash = Rc::clone(&stash);
    let pw_confirm_idx = Rc::clone(&confirm_idx);
    let pw_idx = tree.add_child(
        card,
        SecureInput::new()
            .placeholder("New master password")
            .clear_on(clear_pw)
            .on_submit(move |pw, ctx| {
                if pw.char_count() < MIN_PASSWORD_LEN {
                    set_error(
                        &pw_state,
                        format!("password must be at least {} characters", MIN_PASSWORD_LEN),
                    );
                    return;
                }
                // Stash a copy and clear any prior error so the status
                // line advances to the confirm prompt.
                *pw_stash.borrow_mut() = Some(pw.expose(SecureString::new));
                pw_state.borrow_mut().phase = Phase::Setup { error: None };
                ctx.focus(pw_confirm_idx.get());
            }),
    );

    // Confirm field — Enter compares against the stash, then creates the
    // vault on a match.
    let cf_state = Rc::clone(&state);
    let cf_stash = Rc::clone(&stash);
    let confirm = tree.add_child(
        card,
        SecureInput::new()
            .placeholder("Confirm password")
            .clear_on(clear_cf)
            .on_submit(move |confirm_pw, ctx| {
                let matched = match cf_stash.borrow().as_ref() {
                    Some(first) => first == confirm_pw,
                    None => {
                        // Confirm submitted before the password field —
                        // nudge the user back up rather than comparing
                        // against an empty stash.
                        set_error(&cf_state, "enter your password above first".into());
                        ctx.focus(pw_idx);
                        return;
                    }
                };
                if !matched {
                    set_error(&cf_state, "passwords don't match \u{2014} try again".into());
                    cf_stash.borrow_mut().take(); // zeroize the stale copy
                    clear_pw.bump();
                    clear_cf.bump();
                    ctx.focus(pw_idx);
                    return;
                }
                match create_vault(&cf_state, confirm_pw) {
                    Ok(()) => {
                        cf_stash.borrow_mut().take();
                        let next = Rc::clone(&cf_state);
                        ctx.replace_screen(move |tree| vault_screen::build(tree, next));
                    }
                    Err(e) => {
                        set_error(&cf_state, e);
                        cf_stash.borrow_mut().take();
                        clear_pw.bump();
                        clear_cf.bump();
                        ctx.focus(pw_idx);
                    }
                }
            }),
    );
    confirm_idx.set(confirm);
    tree.focus_initially(pw_idx);

    // Status line: errors in red, otherwise a stage-aware prompt that
    // reads the stash to know whether we're waiting on the first password
    // or its confirmation.
    let status_state = Rc::clone(&state);
    let status_stash = Rc::clone(&stash);
    let color_state = Rc::clone(&state);
    tree.add_child(
        card,
        TextWidget::reactive(move || match &status_state.borrow().phase {
            Phase::Setup { error: Some(e) } => e.clone(),
            Phase::Setup { error: None } => {
                if status_stash.borrow().is_some() {
                    "Re-enter the same password to confirm.".to_string()
                } else {
                    "Choose a master password and press Enter.".to_string()
                }
            }
            _ => String::new(),
        })
        .color(Reactive::derive(move || {
            match &color_state.borrow().phase {
                Phase::Setup { error: Some(_) } => Color::rgb(0.9, 0.4, 0.4),
                _ => Color::rgb(0.7, 0.7, 0.75),
            }
        })),
    );
}

/// Generate a salt, derive the key, and create an empty SQLCipher vault,
/// transitioning `state` into `Unlocked`. Returns a human-readable error
/// string on any failure (config dir, salt write, DB create) so the
/// confirm handler can surface it on the status line.
fn create_vault(state: &Rc<RefCell<AppState>>, password: &SecureString) -> Result<(), String> {
    let Some(paths) = VaultPaths::default_for_app() else {
        return Err("config directory unavailable".into());
    };

    let salt = random_salt();
    paths
        .write_salt(&salt)
        .map_err(|e| format!("failed to write salt: {}", e))?;

    let key = password.expose(|p| derive_key(p.as_bytes(), &salt));

    let storage = VaultStorage::open(&paths.db, &key)
        .map_err(|e| format!("failed to create vault: {}", e))?;

    let mut s = state.borrow_mut();
    s.complete_setup(salt, key, Vec::new(), storage);
    // Materialize the (empty) vault now so a crash before the first
    // auto-save tick still leaves a valid keyed DB — the next launch
    // then sees an existing vault and goes to the lock screen instead of
    // re-running setup.
    s.rewrite_vault_to_storage()
        .map_err(|e| format!("failed to initialize vault: {}", e))?;

    Ok(())
}

fn set_error(state: &Rc<RefCell<AppState>>, msg: String) {
    state.borrow_mut().phase = Phase::Setup { error: Some(msg) };
}
