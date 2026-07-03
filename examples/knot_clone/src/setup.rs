//! Setup screen — faithful reproduction of `Auth/SetupScreen.tsx` (the
//! first-run "Create Vault" flow).
//!
//! Reference layout (Tailwind); canonical state = `docs/screenshots/setup.png`
//! (a valid, "Very strong" password with matching confirm):
//!   - Full-screen page bg, flex center both axes, `p-4`.
//!   - Top-right language `<select>` (`absolute top-4 right-4`) — DEFERRED,
//!     see gap G5 (no absolute-positioning primitive), same as the unlock screen.
//!   - Centered column, `w-full max-w-md`:
//!       - header (`text-center mb-8`): "Knot" `text-4xl font-bold`, subtitle
//!         `text-gray-500`.
//!       - form (`space-y-6`):
//!           - a heading block: "Create Vault" `text-xl font-semibold` + a
//!             `text-sm text-gray-500` description.
//!           - a `space-y-4` field block: master-password field + strength
//!             meter, confirm-password field, and the recovery-key checkbox.
//!           - a full-width blue `py-3` submit.
//!
//! Departures from the reference (both documented inline where they occur):
//!   - the strength meter is rendered *statically* at level 4 ("Very strong",
//!     `bg-emerald-500`) to match the canonical screenshot; the live component
//!     derives level/color from the typed password and hides while it's empty.
//!   - the conditional error banner (`validationError || error`) is omitted —
//!     the canonical state is valid input, so no banner shows. Its chrome needs
//!     no primitive we don't already have.
//!
//! UI-only clone: "Create Vault" just navigates to the placeholder main screen
//! (mirroring the real `setup → unlocked`). The screen is reached via the
//! dev-nav shortcut wired in `main.rs` (Ctrl+2).

use shroud::core::Color;
use shroud::reactive::Reactive;
use shroud::text::FontWeight;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Checkbox, Container, SecureInput, TextWidget};

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
    // unlock screen's convention.
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
        TextWidget::new("A knot only you can untie") // setup.subtitle
            .font_size(16.0)
            .color(tokens::muted()),
    );

    // --- Heading block: "Create Vault" + description ---
    let heading = tree.add_child(content, Container::column().gap(16.0)); // h2 mb-4
    tree.add_child(
        heading,
        TextWidget::new("Create Vault") // setup.title
            .font_size(20.0) // text-xl
            .weight(FontWeight::SEMIBOLD)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        heading,
        TextWidget::new("Set a master password. This password will be used to encrypt your notes.") // setup.description
            .font_size(14.0) // text-sm
            .color(tokens::muted()),
    );

    // --- Field block (space-y-4) ---
    let fields = tree.add_child(content, Container::column().gap(16.0));

    // Master-password field + strength meter beneath it. The column gap (8)
    // covers both the label's `mb-2` and the meter's `mt-2`.
    let pw_group = tree.add_child(fields, Container::column().gap(8.0));
    tree.add_child(
        pw_group,
        TextWidget::new("Master Password") // setup.masterPassword
            .font_size(14.0)
            .weight(FontWeight::MEDIUM)
            .color(tokens::on_surface()),
    );
    // `px-4 py-3 border rounded-lg`: default border tracks `input_border`,
    // `radius(8)` = rounded-lg, `padding_x(16).min_height(48)` = px-4 py-3
    // (FW-17 / G3), same chrome as the unlock field.
    tree.add_child(
        pw_group,
        SecureInput::new()
            .placeholder("8 characters or more") // setup.passwordPlaceholder
            .font_size(16.0)
            .radius(8.0)
            .padding_x(16.0)
            .min_height(48.0),
    );
    strength_meter(tree, pw_group);

    // Confirm-password field.
    let confirm_group = tree.add_child(fields, Container::column().gap(8.0));
    tree.add_child(
        confirm_group,
        TextWidget::new("Confirm Password") // setup.confirmPassword
            .font_size(14.0)
            .weight(FontWeight::MEDIUM)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        confirm_group,
        SecureInput::new()
            .placeholder("Enter again") // setup.confirmPlaceholder
            .font_size(16.0)
            .radius(8.0)
            .padding_x(16.0)
            .min_height(48.0),
    );

    // Recovery-key checkbox (`flex items-center gap-3`). The shroud Checkbox
    // bundles its own box + label; the check fill takes a static Color (G12),
    // so we leave it on the theme default (primary blue when checked).
    tree.add_child(
        fields,
        Checkbox::new("Generate recovery key (recommended)") // setup.generateRecoveryKey
            .checked(true)
            .font_size(14.0),
    );

    // NOTE: the conditional error banner (`validationError || error`) is not
    // reproduced — the canonical state is valid input, so it never shows. It
    // would be a `p-3 rounded-lg` red box with a 1px border:
    //   Container::column().padding(12.0).radius(8.0)
    //       .background(pick(red_100, red-900/50)).border(1.0, pick(red_300, red_700))
    // wrapping `text-sm` `text-red-700 dark:text-red-200` text — all chrome we
    // already have (no new gap).

    // --- Submit (w-full py-3 bg-blue-600) ---
    // No padding/height control on Button (G6); column align-stretch gives the
    // full width and `radius(8)` = rounded-lg. Background is primary blue, so
    // the default hover/press fade to darker blue is already correct.
    tree.add_child(
        content,
        Button::new("Create Vault") // setup.createVault
            .radius(8.0)
            .background(tokens::primary())
            .hover_background(tokens::primary_hover())
            .text_color(Color::WHITE)
            .on_click(|ctx| {
                ctx.replace_screen(crate::main_screen::build);
            }),
    );
}

/// The password strength meter (`mt-2`): four `h-1 flex-1 rounded-full` bars
/// over a `text-xs` label. Rendered statically at level 4 ("Very strong",
/// `bg-emerald-500` bars + `text-green-*` label) to match the canonical
/// screenshot; the live component drives level/color off the typed password.
fn strength_meter(tree: &mut WidgetTree, parent: usize) {
    let wrap = tree.add_child(parent, Container::column().gap(4.0)); // bars mb-1
    let bars = tree.add_child(wrap, Container::row().gap(4.0)); // flex gap-1
    for _ in 0..4 {
        // Level 4 → all four bars filled.
        let color: Reactive<Color> = tokens::emerald_500().into();
        tree.add_child(
            bars,
            Container::row()
                .grow(1.0) // flex-1
                .height(4.0) // h-1
                .radius(2.0) // rounded-full
                .background(color),
        );
    }
    tree.add_child(
        wrap,
        TextWidget::new("Very strong") // password.veryStrong
            .font_size(12.0) // text-xs
            // text-green-600 dark:text-green-500
            .color(tokens::pick(tokens::green_600(), tokens::green_500())),
    );
}
