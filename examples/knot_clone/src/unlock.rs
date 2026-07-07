//! Unlock screen — faithful reproduction of `Auth/UnlockScreen.tsx`.
//!
//! Reference layout (Tailwind):
//!   - Full-screen page bg, flex center both axes, `p-4`.
//!   - Top-right language `<select>` (`absolute top-4 right-4`) — DEFERRED,
//!     see gap G5 (no absolute-positioning primitive).
//!   - Centered content column, `w-full max-w-md`:
//!       - header (`text-center mb-8`): "Knot" `text-4xl font-bold`, subtitle
//!         `text-gray-500`.
//!       - form (`space-y-6`): "Unlock" `text-xl font-semibold`; a labelled
//!         password field (`px-4 py-3 border rounded-lg`); a full-width blue
//!         submit (`py-3 bg-blue-600`); a centered "Forgot password?" link.
//!
//! This is a UI-only clone: the submit just navigates to a placeholder main
//! screen. Friction encountered while building this is logged as it surfaces.

use shroud::core::Color;
use shroud::reactive::Signal;
use shroud::text::FontWeight;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, SecureInput, TextWidget};

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

    // Centered content, full width up to max-w-md (28rem = 448px).
    let content = tree.add_child(
        root,
        Container::column()
            .width_full()
            .max_width(448.0)
            .margin_x_auto()
            .gap(24.0), // space-y-6
    );

    // --- Header (text-center, mb-8) ---
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
        TextWidget::new("Your private notes, locked tight.")
            .font_size(16.0)
            .color(tokens::muted()),
    );

    // --- "Unlock" heading (text-xl font-semibold) ---
    tree.add_child(
        content,
        TextWidget::new("Unlock")
            .font_size(20.0)
            .weight(FontWeight::SEMIBOLD)
            .color(tokens::on_surface()),
    );

    // --- Password field group ---
    let group = tree.add_child(content, Container::column().gap(8.0));
    tree.add_child(
        group,
        TextWidget::new("Master Password")
            .font_size(14.0) // text-sm
            .weight(FontWeight::MEDIUM)
            .color(tokens::on_surface()),
    );
    // The reference `disabled={!password}` on the submit: the Unlock button
    // stays inert (and visually dimmed) until at least one character is typed.
    // `SecureInput` can't hand out its plaintext, but `on_length_change` gives
    // us the character count — enough to drive an "is empty" signal without ever
    // touching the secret. Starts `true` to match the fresh, empty field.
    let pw_empty = Signal::new(true);

    // `px-4 py-3 border rounded-lg`: the default border tracks `input_border`
    // (gray-300 light / gray-700 dark), `.radius(8.0)` matches `rounded-lg`
    // (G2), and `.padding_x(16.0).min_height(48.0)` now hits `px-4` + the 48px
    // control height (FW-17 / G3 — previously the fixed internal padding kept it
    // from `px-4 py-3`).
    let input_idx = tree.add_child(
        group,
        SecureInput::new()
            .placeholder("Enter your master password")
            .font_size(16.0)
            .radius(8.0)
            .padding_x(16.0)
            .min_height(48.0)
            .on_length_change(move |n| pw_empty.set(n == 0)),
    );
    tree.focus_initially(input_idx);

    // --- Submit ---
    // Reference: `bg-blue-600 hover:bg-blue-700 disabled:bg-blue-800
    // disabled:cursor-not-allowed ... text-white`. The disabled state recolors
    // the fill to the *darker* blue-800 (not a half-alpha dim) and keeps the
    // label white — so it needs both `disabled_background` and
    // `disabled_text_color`. `.disabled(pw_empty)` also makes the button inert
    // and drops it from the Tab order until a password is typed. No
    // padding/height control on Button (gap G6); column align-stretch gives the
    // full width.
    tree.add_child(
        content,
        Button::new("Unlock")
            .radius(8.0)
            .background(tokens::primary())
            .hover_background(tokens::primary_hover())
            .text_color(Color::WHITE)
            .disabled(pw_empty)
            .disabled_background(tokens::blue_800())
            .disabled_text_color(Color::WHITE)
            .on_click(|ctx| {
                ctx.replace_screen(crate::main_screen::build);
            }),
    );

    // --- "Forgot password?" link (centered, text-sm text-gray-500) ---
    let link_row = tree.add_child(content, Container::row().justify_center());
    tree.add_child(
        link_row,
        Button::new("Forgot password?")
            .background(Color::TRANSPARENT)
            // Text-only link (`text-gray-500 hover:text-gray-700`): transparent
            // fills so no box appears, and the label darkens on hover via
            // `hover_text_color` (FW addition this branch) — that color change
            // is what reads as "this is clickable".
            .hover_background(Color::TRANSPARENT)
            .press_background(Color::TRANSPARENT)
            .text_color(tokens::muted())
            .hover_text_color(tokens::pick(tokens::gray_700(), tokens::gray_300()))
            .font_size(14.0)
            // Real UnlockScreen: "Forgot password?" → showRecoveryScreen().
            .on_click(|ctx| {
                ctx.replace_screen(crate::recovery::build);
            }),
    );
}
