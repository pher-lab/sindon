//! Lock screen — centered card with the master-password field.
//!
//! On submit, derives the master key, attempts to decrypt every entry in the
//! vault, and on success transitions to the vault screen with the resident
//! key + decrypted notes captured in `Phase::Unlocked`.

use std::cell::RefCell;
use std::rc::Rc;

use shroud::core::Color;
use shroud::reactive::Reactive;
use shroud::security::SecureString;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Container, SecureInput, TextWidget};

use crate::crypto::derive_key;
use crate::state::{AppState, Phase};
use crate::{DEMO_PASSWORD, vault_screen};

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

    tree.add_child(
        card,
        TextWidget::new(format!("Master password (hint: {}):", DEMO_PASSWORD)),
    );

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
            // Unlocked is unreachable here — on success the handler queues a
            // replace_screen, so by the next paint we're already off this
            // screen. Render an empty string defensively.
            Phase::Unlocked { .. } => String::new(),
        })
        .color(Reactive::Static(Color::rgb(0.7, 0.7, 0.75))),
    );
}

/// Try to unlock the vault with `master`. Returns `true` if every note
/// decrypted successfully; the caller then queues the screen transition.
/// On failure, writes the error to `Phase::Locked.error` so the status text
/// updates on the next paint.
fn try_unlock(state: &Rc<RefCell<AppState>>, master: &SecureString) -> bool {
    let key = master.expose(|m| derive_key(m.as_bytes(), &state.borrow().salt));

    let notes = match state.borrow().try_decrypt_all(&key) {
        Some(notes) => notes,
        None => {
            state.borrow_mut().phase = Phase::Locked {
                error: Some("wrong master password".into()),
            };
            return false;
        }
    };

    let selected = notes.first().map(|n| n.id);
    state.borrow_mut().phase = Phase::Unlocked {
        key,
        notes,
        selected,
    };
    true
}
