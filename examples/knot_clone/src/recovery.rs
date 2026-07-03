//! Recovery screen — faithful reproduction of `Auth/RecoveryScreen.tsx` (the
//! "Recover Vault" flow reached from the unlock screen's "Forgot password?").
//!
//! Reference layout (Tailwind). The canonical state is the screen's *initial*
//! render — every field empty — because both the strength meter and the error
//! banner are conditional (`newPassword.length > 0` / `validationError ||
//! error`) and so are absent on landing:
//!   - Full-screen page bg, flex center both axes, `p-4`.
//!   - Centered column, `w-full max-w-md`:
//!       - header (`text-center mb-8`): "Knot" `text-4xl font-bold`, subtitle
//!         `text-gray-500`.
//!       - form (`space-y-6`):
//!           - a heading block: "Recover Vault" `text-xl font-semibold` + a
//!             `text-sm text-gray-500` description.
//!           - a `space-y-4` field block: the recovery-key `<textarea>`, the
//!             new-password field (+ meter, hidden while empty), and the
//!             confirm-password field.
//!           - a full-width blue `py-3` submit ("Recover Vault").
//!           - a centered "Back" text link (`text-sm text-gray-500
//!             hover:text-gray-700`).
//!
//! New vocabulary this slice exercises (vs. Unlock/Setup): the recovery-key
//! `<textarea resize-none h-24>` — the first *multi-line* input in the clone.
//! Mapped to `Input::multiline()` (a plain, visible field — the React source
//! uses a bare `<textarea>`, not a password box). See the field for the one
//! sizing nuance (`min_height` floor vs. `resize-none`'s fixed height).
//!
//! Departures (documented inline where they occur):
//!   - the strength meter is omitted — canonical is an empty password, so the
//!     live component doesn't render it. Its chrome is already exercised in
//!     `setup.rs` (`strength_meter`), so nothing new is skipped.
//!   - the conditional error banner (`validationError || error`) is omitted
//!     for the same reason; its `p-3 rounded-lg` red-box chrome needs no
//!     primitive we lack (see the note in `setup.rs`).
//!
//! UI-only clone: "Recover Vault" navigates to the placeholder main screen
//! (mirroring the real `recover → unlocked`); "Back" returns to the unlock
//! screen. Reached via the dev-nav shortcut in `main.rs` (Ctrl+4).

use shroud::core::Color;
use shroud::text::FontWeight;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, Input, SecureInput, TextWidget};

use crate::tokens;

pub fn build(tree: &mut WidgetTree) {
    // Outer: page background, centered, p-4 (16px).
    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .background(tokens::background())
            .padding(16.0)
            .justify_center(),
    );

    // Centered content, full width up to max-w-md (28rem = 448px). `space-y-6`
    // (24) is the form rhythm; the header's `mb-8` folds into it, matching the
    // unlock / setup convention.
    let content = tree.add_child(
        root,
        Container::column()
            .width_full()
            .max_width(448.0)
            .margin_x_auto()
            .gap(24.0),
    );

    // --- Header (text-center, mb-8): "Knot" + subtitle ---
    let header = tree.add_child(content, Container::column().align_center().gap(8.0));
    tree.add_child(
        header,
        TextWidget::new("Knot")
            .font_size(36.0) // text-4xl
            .weight(FontWeight::BOLD)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        header,
        TextWidget::new("A knot only you can untie") // recovery.subtitle
            .font_size(16.0)
            .color(tokens::muted()),
    );

    // --- Heading block: "Recover Vault" + description ---
    let heading = tree.add_child(content, Container::column().gap(16.0)); // h2 mb-4
    tree.add_child(
        heading,
        TextWidget::new("Recover Vault") // recovery.title
            .font_size(20.0) // text-xl
            .weight(FontWeight::SEMIBOLD)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        heading,
        TextWidget::new("Enter your recovery key (12 words) and set a new password.") // recovery.description
            .font_size(14.0) // text-sm
            .color(tokens::muted()),
    );

    // --- Field block (space-y-4) ---
    let fields = tree.add_child(content, Container::column().gap(16.0));

    // Recovery-key `<textarea resize-none h-24>` — the clone's first multi-line
    // input. `Input::multiline()` (plain/visible, matching the bare `<textarea>`;
    // it is *not* a password box). `px-4 py-3` → `padding_x(16).padding_y(12)`,
    // `rounded-lg` → `radius(8)`, default border tracks `input_border`.
    //
    // NUANCE: `h-24` is a *fixed* 96px box that scrolls its content; `min_height`
    // is a *floor*, so an empty field renders at exactly 96px (identical here)
    // but would grow past it once you type >4 rows, where the real `resize-none`
    // textarea would scroll instead. shroud's fixed-viewport multiline is
    // `height_full()`, which fills the *parent*, so an exact fixed-pixel
    // scrolling box needs a wrapper — a small dimension-API gap (G3 family),
    // immaterial to this at-rest snapshot.
    let key_group = tree.add_child(fields, Container::column().gap(8.0)); // label mb-2
    tree.add_child(
        key_group,
        TextWidget::new("Recovery Key") // recovery.recoveryKey
            .font_size(14.0)
            .weight(FontWeight::MEDIUM)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        key_group,
        Input::new()
            .multiline()
            .placeholder("Enter 12 words separated by spaces") // recovery.recoveryKeyPlaceholder
            .font_size(16.0)
            .radius(8.0)
            .padding_x(16.0)
            .padding_y(12.0)
            .min_height(96.0), // h-24
    );

    // New-password field. The strength meter (`newPassword.length > 0`) is
    // omitted for the empty canonical — see the module note.
    let pw_group = tree.add_child(fields, Container::column().gap(8.0));
    tree.add_child(
        pw_group,
        TextWidget::new("New Password") // recovery.newPassword
            .font_size(14.0)
            .weight(FontWeight::MEDIUM)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        pw_group,
        SecureInput::new()
            .placeholder("8 characters or more") // recovery.newPasswordPlaceholder
            .font_size(16.0)
            .radius(8.0)
            .padding_x(16.0)
            .min_height(48.0), // px-4 py-3
    );

    // Confirm-password field.
    let confirm_group = tree.add_child(fields, Container::column().gap(8.0));
    tree.add_child(
        confirm_group,
        TextWidget::new("Confirm Password") // recovery.confirmPassword
            .font_size(14.0)
            .weight(FontWeight::MEDIUM)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        confirm_group,
        SecureInput::new()
            .placeholder("Enter again") // recovery.confirmPlaceholder
            .font_size(16.0)
            .radius(8.0)
            .padding_x(16.0)
            .min_height(48.0),
    );

    // NOTE: the conditional error banner (`validationError || error`) is not
    // reproduced — the canonical state is the empty initial form, so it never
    // shows. Its chrome (`p-3 rounded-lg` red box + 1px border) needs no
    // primitive we lack; see the same note in `setup.rs`.

    // --- Submit (w-full py-3 bg-blue-600) ---
    // No padding/height control on Button (G6); column align-stretch gives the
    // full width and `radius(8)` = rounded-lg. Background is primary blue, so
    // the default hover/press fade to darker blue is already correct.
    tree.add_child(
        content,
        Button::new("Recover Vault") // recovery.recover
            .radius(8.0)
            .background(tokens::primary())
            .hover_background(tokens::primary_hover())
            .text_color(Color::WHITE)
            .on_click(|ctx| {
                ctx.replace_screen(crate::main_screen::build);
            }),
    );

    // --- "Back" link (centered, text-sm text-gray-500) ---
    // Same text-only link idiom as the unlock screen's "Forgot password?":
    // transparent fills so no box shows, and the label darkens on hover via
    // `hover_text_color`.
    let back_row = tree.add_child(content, Container::row().justify_center());
    tree.add_child(
        back_row,
        Button::new("Back") // recovery.back
            .background(Color::TRANSPARENT)
            .hover_background(Color::TRANSPARENT)
            .press_background(Color::TRANSPARENT)
            .text_color(tokens::muted())
            .hover_text_color(tokens::pick(tokens::gray_700(), tokens::gray_300()))
            .font_size(14.0)
            .on_click(|ctx| {
                ctx.replace_screen(crate::unlock::build);
            }),
    );
}
