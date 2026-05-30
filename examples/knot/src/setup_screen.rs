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

use shroud::reactive::Reactive;
use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, ClearTrigger, Container, SecureInput, TextWidget};
use zeroize::Zeroizing;

use crate::crypto::{derive_key, generate_dek, random_salt, recovery, wrap_dek};
use crate::settings;
use crate::state::{AppState, Phase};
use crate::storage::{VaultPaths, VaultStorage};
use crate::vault_screen;

/// Minimum master-password length (characters). A typo guard, not a
/// strength policy — recovery via the BIP39 key is the real safety net
/// against a forgotten password.
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
        TextWidget::new("Create a master password for your vault."),
    );

    tree.add_child(
        card,
        TextWidget::new(format!(
            "At least {} characters. You'll get a recovery key next in case you forget it.",
            MIN_PASSWORD_LEN
        ))
        .color(settings::on_surface_variant()),
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
                    Ok(mnemonic) => {
                        cf_stash.borrow_mut().take();
                        let next = Rc::clone(&cf_state);
                        // Show the recovery key once before entering the
                        // vault. The mnemonic moves into the closure and is
                        // zeroized when that reveal screen is built/dropped.
                        ctx.replace_screen(move |tree| {
                            build_recovery_reveal(tree, next, mnemonic)
                        });
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
            let theme = settings::current_theme();
            match &color_state.borrow().phase {
                Phase::Setup { error: Some(_) } => theme.colors.error,
                _ => theme.colors.on_surface_variant,
            }
        })),
    );
}

/// Build a fresh envelope-encrypted vault and transition `state` into
/// `Unlocked`. On success returns the freshly-generated BIP39 recovery
/// mnemonic (held zeroizing) so the caller can show it once. Returns a
/// human-readable error string on any failure (config dir, file write, DB
/// create) so the confirm handler can surface it on the status line.
///
/// Envelope scheme: a random DEK keys the DB and notes; it is wrapped
/// under both the password-derived KEK (`dek.enc`) and the recovery KEK
/// (`recovery.enc`). The order — write the wrappings *before* opening the
/// DB and completing setup — means a crash mid-setup leaves no half-vault
/// the next launch would mistake for complete (`vault_exists` requires the
/// DB too, which is created last).
fn create_vault(
    state: &Rc<RefCell<AppState>>,
    password: &SecureString,
) -> Result<Zeroizing<String>, String> {
    let Some(paths) = VaultPaths::default_for_app() else {
        return Err("config directory unavailable".into());
    };

    let salt = random_salt();
    let dek = generate_dek();

    // Wrap the DEK under the password KEK.
    let pw_kek = password.expose(|p| derive_key(p.as_bytes(), &salt));
    let pw_wrapped = wrap_dek(&pw_kek, &dek);

    // Wrap the same DEK under a fresh recovery KEK.
    let mnemonic = recovery::generate_mnemonic();
    let rec_kek = recovery::key_to_kek(&mnemonic)
        .ok_or_else(|| "failed to derive recovery key".to_string())?;
    let rec_wrapped = wrap_dek(&rec_kek, &dek);

    paths
        .write_salt(&salt)
        .map_err(|e| format!("failed to write salt: {}", e))?;
    paths
        .write_wrapped_dek(&pw_wrapped)
        .map_err(|e| format!("failed to write key file: {}", e))?;
    paths
        .write_wrapped_recovery(&rec_wrapped)
        .map_err(|e| format!("failed to write recovery file: {}", e))?;

    // Open (create) the DB keyed with the DEK — done last so it's the
    // final file to appear; see the order rationale above.
    let storage = VaultStorage::open(&paths.db, &dek)
        .map_err(|e| format!("failed to create vault: {}", e))?;

    let mut s = state.borrow_mut();
    s.complete_setup(salt, dek, Vec::new(), storage);
    // Materialize the (empty) vault now so a crash before the first
    // auto-save tick still leaves a valid keyed DB — the next launch
    // then sees an existing vault and goes to the lock screen instead of
    // re-running setup.
    s.rewrite_vault_to_storage()
        .map_err(|e| format!("failed to initialize vault: {}", e))?;

    Ok(mnemonic)
}

fn set_error(state: &Rc<RefCell<AppState>>, msg: String) {
    state.borrow_mut().phase = Phase::Setup { error: Some(msg) };
}

/// One-time recovery-key reveal, shown immediately after the vault is
/// created (state is already `Unlocked` here). Displays the 12 words for
/// the user to write down, then a Continue button drops into the vault.
/// The `mnemonic` is consumed: its words are copied into the displayed
/// text widgets and the `Zeroizing<String>` is wiped when this function
/// returns. Screen capture is already blocked app-wide
/// (`capture_prevention(true)` in `main`), so the plaintext words on
/// screen aren't exposed to screenshots/recording.
fn build_recovery_reveal(
    tree: &mut WidgetTree,
    state: Rc<RefCell<AppState>>,
    mnemonic: Zeroizing<String>,
) {
    let words: Vec<&str> = mnemonic.split_whitespace().collect();

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
            .width(520.0)
            .margin_x_auto()
            .padding(32.0)
            .gap(16.0)
            .background(settings::surface())
            .radius(16.0),
    );

    tree.add_child(card, TextWidget::new("Your recovery key").font_size(28.0));
    tree.add_child(
        card,
        TextWidget::new(
            "If you forget your password, these 12 words are the only way back into \
             your vault. Write them down and store them somewhere safe \u{2014} they're \
             shown only once.",
        )
        .color(settings::on_surface_variant()),
    );

    // 12 words in a 3-row x 4-column grid of fixed-width cells so the
    // columns line up. Fixed widths avoid the wrapped-text overlap that
    // percent-width columns can hit (see the margin_x_auto fix memo).
    let grid = tree.add_child(
        card,
        Container::column()
            .gap(8.0)
            .padding(16.0)
            .background(settings::background())
            .radius(8.0),
    );
    for (row_idx, row_words) in words.chunks(4).enumerate() {
        let row = tree.add_child(grid, Container::row().gap(12.0));
        for (col, word) in row_words.iter().enumerate() {
            let n = row_idx * 4 + col + 1;
            let cell = tree.add_child(row, Container::row().width(108.0).gap(6.0));
            tree.add_child(
                cell,
                TextWidget::new(format!("{}.", n)).color(settings::on_surface_variant()),
            );
            tree.add_child(
                cell,
                TextWidget::new((*word).to_string()).color(settings::on_surface()),
            );
        }
    }

    tree.add_child(
        card,
        TextWidget::new(
            "Anyone who has these words can open your vault. Never share them or store \
             them with your password.",
        )
        .color(settings::warning()),
    );

    let next = Rc::clone(&state);
    tree.add_child(
        card,
        Button::new("I've saved it \u{2014} open my vault")
            .radius(8.0)
            .on_click(move |ctx| {
                let next = Rc::clone(&next);
                ctx.replace_screen(move |tree| vault_screen::build(tree, next));
            }),
    );
}
