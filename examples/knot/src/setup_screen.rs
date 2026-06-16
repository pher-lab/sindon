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

use shroud::platform::FileDialog;
use shroud::reactive::Reactive;
use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, ClearTrigger, Container, SecureInput, TextWidget};
use zeroize::Zeroizing;

use crate::crypto::{derive_key, generate_dek, random_salt, recovery, wrap_dek};
use crate::i18n::{self, Key};
use crate::state::{AppState, Note, Phase};
use crate::storage::{VaultPaths, VaultStorage};
use crate::vault_screen;
use crate::{notice, recovery_pdf, settings};

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
        TextWidget::reactive(|| i18n::tr(Key::SetupDescription).to_string()),
    );

    tree.add_child(
        card,
        TextWidget::reactive(|| {
            i18n::tr(Key::SetupHint).replace("{n}", &MIN_PASSWORD_LEN.to_string())
        })
        .color(settings::on_surface_variant()),
    );

    // Password field — Enter validates length, stashes a copy, advances.
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
            .placeholder(i18n::tr(Key::ConfirmPasswordPlaceholder))
            .clear_on(clear_cf)
            .on_submit(move |confirm_pw, ctx| {
                let matched = match cf_stash.borrow().as_ref() {
                    Some(first) => first == confirm_pw,
                    None => {
                        // Confirm submitted before the password field —
                        // nudge the user back up rather than comparing
                        // against an empty stash.
                        set_error(&cf_state, i18n::tr(Key::ConfirmFirstSetup).to_string());
                        ctx.focus(pw_idx);
                        return;
                    }
                };
                if !matched {
                    set_error(&cf_state, i18n::tr(Key::PasswordsMismatch).to_string());
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
                        ctx.replace_screen(move |tree| build_recovery_reveal(tree, next, mnemonic));
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
                    i18n::tr(Key::SetupConfirmPrompt).to_string()
                } else {
                    i18n::tr(Key::SetupChoosePrompt).to_string()
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
        .ok_or_else(|| i18n::tr(Key::ErrDeriveRecovery).to_string())?;
    let rec_wrapped = wrap_dek(&rec_kek, &dek);

    paths
        .write_salt(&salt)
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrWriteSaltPrefix), e))?;
    paths
        .write_wrapped_dek(&pw_wrapped)
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrWriteKeyFilePrefix), e))?;
    paths
        .write_wrapped_recovery(&rec_wrapped)
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrWriteRecoveryPrefix), e))?;

    // Open (create) the DB keyed with the DEK — done last so it's the
    // final file to appear; see the order rationale above.
    let storage = VaultStorage::open(&paths.db, &dek)
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrCreateVaultPrefix), e))?;

    let mut s = state.borrow_mut();
    s.complete_setup(salt, dek, welcome_notes(), storage);
    // Materialize the vault now (with its seeded welcome note) so a crash
    // before the first auto-save tick still leaves a valid keyed DB — the next
    // launch then sees an existing vault and goes to the lock screen instead of
    // re-running setup.
    s.rewrite_vault_to_storage()
        .map_err(|e| format!("{}{}", i18n::tr(Key::ErrInitVaultPrefix), e))?;

    Ok(mnemonic)
}

/// The single note a brand-new vault is seeded with, so the first thing the
/// user sees after setup is a short tour rather than an empty editor. It's an
/// ordinary note (id 1, no tags, unpinned) — the user can edit or delete it
/// like any other. Written in Markdown so toggling Preview shows it rendered.
fn welcome_notes() -> Vec<Note> {
    vec![Note {
        id: 1,
        title: "Welcome to Knot".to_string(),
        body: WELCOME_BODY.to_string(),
        tags: Vec::new(),
        pinned: false,
        deleted_at: None,
    }]
}

/// Body of the seeded welcome note. Plain Markdown — kept terse so it reads
/// well both in the editor and in the preview pane.
const WELCOME_BODY: &str = "\
# Welcome to Knot 🔒

This is your private vault. Everything in it is encrypted on disk — only your \
master password (or your recovery key) can open it.

## Getting started

- **New note** — click **+ New** in the sidebar.
- **Markdown** — write in Markdown, then toggle **Preview** to see it rendered.
- **Tags** — add tags under the title to organize and filter notes.
- **Search** — press **Ctrl+F** to search across every note.
- **Images** — drag one in, paste from the clipboard, or use the **Image** button.
- **Links** — link between notes with `[[Note title]]`.
- **Pin & sort** — star a note to keep it on top, or change the sidebar **Sort** order.

## Staying safe

- **Lock** the vault (or press **Ctrl+L**) whenever you step away — it re-encrypts \
and drops the key.
- Knot also auto-locks after a period of inactivity. Adjust it in **⚙ Settings**.
- Keep your **recovery key** somewhere safe — it is the only way back in if you \
forget your password.

You can delete this note once you've read it. Happy writing!
";

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

    // A copy of the phrase for the "Save as PDF" button. It lives in the
    // button closure for exactly as long as this reveal screen does — the same
    // exposure window as the words already shown on screen — and is zeroized
    // when the screen is replaced (Continue) and the widget tree drops.
    let phrase_for_pdf = Zeroizing::new(mnemonic.trim().to_string());

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

    tree.add_child(
        card,
        TextWidget::reactive(|| i18n::tr(Key::RecoveryRevealTitle).to_string()).font_size(28.0),
    );
    tree.add_child(
        card,
        TextWidget::reactive(|| i18n::tr(Key::RecoveryRevealDescription).to_string())
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
        TextWidget::reactive(|| i18n::tr(Key::RecoveryRevealWarning).to_string())
            .color(settings::warning()),
    );

    // Actions row: save the key as a printable PDF, or continue into the vault.
    let actions = tree.add_child(card, Container::row().gap(12.0));

    // Save-as-PDF (secondary). Writes the 12 words to a user-chosen
    // `knot-recovery-key.pdf` via the dependency-free `recovery_pdf` writer.
    // The bytes hold the plaintext key, so `render` returns them zeroizing and
    // they're wiped the moment this closure's temporary drops after the write.
    tree.add_child(
        actions,
        Button::reactive_label(|| i18n::tr(Key::RecoverySavePdf).to_string())
            .radius(8.0)
            .on_click(move |_ctx| {
                let Some(path) = FileDialog::new()
                    .title(i18n::tr(Key::DialogSaveRecoveryPdf))
                    .filter("PDF", &["pdf"])
                    .file_name("knot-recovery-key.pdf".to_string())
                    .save_file()
                else {
                    return;
                };
                let pdf = recovery_pdf::render(phrase_for_pdf.as_str());
                if let Err(e) = std::fs::write(&path, &*pdf) {
                    notice::show(format!("{}{e}", i18n::tr(Key::ErrRecoveryPdfPrefix)));
                }
            }),
    );

    // Continue (primary) — drops into the vault.
    let next = Rc::clone(&state);
    tree.add_child(
        actions,
        Button::reactive_label(|| i18n::tr(Key::RecoveryRevealDone).to_string())
            .radius(8.0)
            .on_click(move |ctx| {
                let next = Rc::clone(&next);
                ctx.replace_screen(move |tree| vault_screen::build(tree, next));
            }),
    );
}
