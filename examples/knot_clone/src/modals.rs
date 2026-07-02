//! Modal slice (slice 4) — the four dialogs that float over the main screen:
//! `ConfirmDialog`, `ChangePasswordModal`, `BackupSettingsModal`, and
//! `RestoreBackupModal`. Each is a `fixed inset-0 bg-black/50 flex items-center
//! justify-center` scrim in React with a centered card; here every one is a
//! [`LayerOptions::modal`] layer (scrim + `ViewportCenter` + dismiss on
//! outside-click / Escape), which maps 1:1 onto the outer div.
//!
//! Entry points are the two text rows in the settings dropdown ("Backup
//! Settings" / "Change Master Password"). The nesting mirrors React: Backup
//! Settings opens Restore *on top* (two stacked scrims), and a Restore row
//! opens a Confirm dialog on top of that — a genuine three-deep layer-stack
//! probe.
//!
//! Gaps this slice surfaces (logged in `docs/knot-ui-repro-gaps.md`):
//!   - **G18 (new)** no drop-shadow / elevation primitive. Every modal is
//!     `shadow-xl`, which is what lifts the card off the scrim; shroud has no
//!     shadow at all, so the cards sit flat. There is also no viewport-relative
//!     sizing (`max-h-[80vh]`): the Restore card hardcodes `max_height(576)`
//!     (80% of the 720px window) instead.
//!   - **G4** asymmetric padding — modal sections are `px-6 py-4`, fields
//!     `px-3 py-2` / `px-2 py-1`; `Container::padding` is uniform, so these are
//!     approximated (the inputs themselves do get `padding_x/y` from FW-17).
//!   - **G6** `Button` has no height/`disabled` styling — the `py-2` footer
//!     buttons and every `disabled:opacity-50` / `disabled:bg-blue-800` state
//!     are out of reach; footer buttons get their width from `grow`.
//!   - **G12** `Input`/`SecureInput`/`Checkbox` take a *static* `Color` for
//!     fill/border/label, not `Reactive<Color>` — so the modal fields keep the
//!     default theme-tracking chrome (white/gray-800 vs React's white/gray-700,
//!     a minor shade drift) rather than pinning a color that wouldn't follow
//!     the Ctrl+D toggle.
//!   - **grid** `grid grid-cols-2 gap-3` (interval / retention) has no layout
//!     primitive; faked with a `row` of two `grow(1.0)` columns.

use shroud::core::Color;
use shroud::layout::Justify;
use shroud::reactive::{Reactive, Signal};
use shroud::text::FontWeight;
use shroud::widgets::layer::LayerOptions;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{
    Button, Checkbox, Container, EventContext, Input, ScrollView, SecureInput, TextWidget,
};

use crate::tokens;

// --- Entry points (wired from the settings dropdown) -------------------------

/// The Change-Master-Password dialog (`ChangePasswordModal.tsx`): `max-w-sm`,
/// three password fields, a strength meter under the new-password field, and a
/// Cancel / Change footer. Shown in its default (form) state — the success
/// sub-state (a centered green check) is a state the static clone can't reach.
pub fn open_change_password(ctx: &mut EventContext) {
    // `bg-white dark:bg-gray-800 rounded-lg shadow-xl w-full max-w-sm mx-4` with
    // an inner `p-6`; the `shadow-xl` has no counterpart (G18). `space-y-4`
    // between the title and the fields is a uniform `gap(16)`.
    let card = card_column(384.0).padding(24.0).gap(16.0);
    ctx.push_layer(LayerOptions::modal(), card, |tree, root| {
        title(tree, root, "Change Master Password");

        field(tree, root, "Current Password", "Enter current password");

        // New password field carries the strength meter beneath its input.
        let new_group = field(tree, root, "New Password", "Enter new password");
        strength_meter(tree, new_group);

        field(tree, root, "Confirm New Password", "Re-enter new password");

        // Footer (`flex gap-2 pt-2`): two `flex-1 py-2 rounded-lg` buttons.
        let footer = tree.add_child(root, Container::row().gap(8.0));
        tree.add_child(
            footer,
            btn_secondary("Cancel", 8.0)
                .grow(1.0)
                .on_click(|c| c.pop_top_layer()),
        );
        tree.add_child(
            footer,
            btn_primary("Change Password", 8.0)
                .grow(1.0)
                .on_click(|c| c.pop_top_layer()),
        );
    });
}

/// The Backup-Settings dialog (`BackupSettingsModal.tsx`): `max-w-md`, an
/// enable checkbox, a directory row, a two-column interval/retention grid, a
/// last-backup line + explanatory note, and a wrapping footer whose Cancel is
/// pushed right by an `ml-auto` spacer. "Restore" opens the Restore dialog on
/// top (this modal stays mounted underneath).
pub fn open_backup_settings(ctx: &mut EventContext) {
    let card = card_column(448.0).padding(24.0).gap(16.0);
    ctx.push_layer(LayerOptions::modal(), card, |tree, root| {
        title(tree, root, "Backup Settings");

        // `<label><input type=checkbox className="rounded"/> …</label>`.
        // Checkbox keeps its default (theme-tracking) chrome — `label_color`
        // and `check_color` take a static `Color` (G12), so we don't pin them.
        tree.add_child(
            root,
            Checkbox::new("Enable automatic backups")
                .checked(true)
                .font_size(14.0),
        );

        // Directory row: a `text-xs` label over a [readonly path field + a
        // "Choose…" button]. The field is a real `<div>` (not an input), so it
        // *can* take a reactive `bg-gray-100 dark:bg-gray-700` faithfully.
        let dir_sec = tree.add_child(root, Container::column().gap(4.0));
        tree.add_child(
            dir_sec,
            TextWidget::new("Backup folder")
                .font_size(12.0)
                .color(tokens::muted()),
        );
        let dir_row = tree.add_child(dir_sec, Container::row().align_center().gap(8.0));
        let path_field = tree.add_child(
            dir_row,
            Container::row()
                .grow(1.0)
                .padding(6.0) // ≈ px-2 py-1 (G4)
                .radius(4.0)
                .background(tokens::pick(tokens::gray_100(), tokens::gray_700()))
                .border(1.0, field_border()),
        );
        tree.add_child(
            path_field,
            TextWidget::new("C:\\Users\\you\\KnotBackups")
                .font_size(14.0)
                .truncate(true)
                .color(label_fg()),
        );
        tree.add_child(
            dir_row,
            btn_secondary("Choose\u{2026}", 4.0) // Choose…
                .font_size(14.0)
                .on_click(|_c| {}),
        );

        // `grid grid-cols-2 gap-3` — no grid primitive, so a row of two grow
        // columns, each a `text-xs` label over a `px-2 py-1` number input.
        let grid = tree.add_child(root, Container::row().gap(12.0));
        number_col(tree, grid, "Every N days", "7");
        number_col(tree, grid, "Keep N backups", "30");

        // Last-backup line + the explanatory note (both `text-xs text-gray-500`).
        tree.add_child(
            root,
            TextWidget::new("Last backup: Apr 27, 2026, 09:14")
                .font_size(12.0)
                .color(tokens::muted()),
        );
        tree.add_child(
            root,
            TextWidget::new(
                "Backups are encrypted with your master password. Store them somewhere safe.",
            )
            .font_size(12.0)
            .color(tokens::muted()),
        );

        // Footer (`flex flex-wrap gap-2 pt-2`): Run Now / Restore, then an
        // `ml-auto` spacer pushing Cancel to the right edge.
        let footer = tree.add_child(
            root,
            Container::row().align_center().gap(8.0).flex_wrap(true),
        );
        tree.add_child(
            footer,
            btn_primary("Back up now", 4.0)
                .font_size(14.0)
                .on_click(|_c| {}),
        );
        tree.add_child(
            footer,
            btn_secondary("Restore\u{2026}", 4.0)
                .font_size(14.0)
                .on_click(open_restore_backup),
        );
        tree.add_child(footer, Container::row().grow(1.0)); // ml-auto spacer
        tree.add_child(
            footer,
            btn_secondary("Cancel", 4.0)
                .font_size(14.0)
                .on_click(|c| c.pop_top_layer()),
        );
    });
}

/// The Restore-from-Backup dialog (`RestoreBackupModal.tsx`): `max-w-md
/// max-h-[80vh] flex flex-col` — a sticky `border-b` header, a `flex-1
/// overflow-y-auto` list of backups, and a sticky `border-t` footer. This is
/// the slice's headline layout probe: does a `max_height`-capped column with a
/// `grow` [`ScrollView`] in the middle actually clip and scroll while the
/// header/footer stay pinned? Enough dummy rows are emitted to overflow.
pub fn open_restore_backup(ctx: &mut EventContext) {
    // `max-h-[80vh]` → no viewport-relative sizing (G18); hardcode 80% of the
    // known 720px window. No inner padding on the card: each section owns its
    // own `px-6`.
    let card = card_column(448.0).max_height(576.0);
    ctx.push_layer(LayerOptions::modal(), card, |tree, root| {
        // Sticky header (`px-6 py-4 border-b`).
        let header = tree.add_child(
            root,
            Container::column()
                .padding(16.0)
                .border_bottom(1.0, section_border()),
        );
        tree.add_child(
            header,
            TextWidget::new("Restore from Backup")
                .font_size(18.0)
                .weight(FontWeight::SEMIBOLD)
                .color(tokens::on_surface()),
        );

        // Scrollable body (`flex-1 overflow-y-auto px-6 py-4`, list `space-y-2`).
        let body = tree.add_child(root, ScrollView::new().grow(1.0).padding(16.0).gap(8.0));
        for b in BACKUPS {
            backup_row(tree, body, b);
        }

        // Sticky footer (`px-6 py-3 border-t flex justify-end`).
        let footer = tree.add_child(
            root,
            Container::row()
                .padding(12.0)
                .justify(Justify::End)
                .border_top(1.0, section_border()),
        );
        tree.add_child(
            footer,
            btn_secondary("Cancel", 4.0)
                .font_size(14.0)
                .on_click(|c| c.pop_top_layer()),
        );
    });
}

// --- ConfirmDialog -----------------------------------------------------------

/// The shared confirm dialog (`ConfirmDialog.tsx`): `max-w-sm p-6`, a title, a
/// message, and a Cancel / Confirm footer. `variant="danger"` paints the
/// confirm red. The clone's confirm/cancel both just pop the dialog.
fn open_confirm(
    ctx: &mut EventContext,
    title_text: &'static str,
    message: &'static str,
    confirm_label: &'static str,
    danger: bool,
) {
    let card = card_column(384.0).padding(24.0);
    ctx.push_layer(LayerOptions::modal(), card, move |tree, root| {
        // Title (`text-lg font-semibold mb-2`).
        tree.add_child(
            root,
            TextWidget::new(title_text)
                .font_size(18.0)
                .weight(FontWeight::SEMIBOLD)
                .color(tokens::on_surface()),
        );
        v_space(tree, root, 8.0); // mb-2
        // Message (`text-sm text-gray-600 mb-6`).
        tree.add_child(
            root,
            TextWidget::new(message)
                .font_size(14.0)
                .color(tokens::muted()),
        );
        v_space(tree, root, 24.0); // mb-6

        let footer = tree.add_child(root, Container::row().gap(8.0));
        tree.add_child(
            footer,
            btn_secondary("Cancel", 8.0)
                .grow(1.0)
                .on_click(|c| c.pop_top_layer()),
        );
        let confirm = if danger {
            btn_danger(confirm_label, 8.0)
        } else {
            btn_primary(confirm_label, 8.0)
        };
        tree.add_child(footer, confirm.grow(1.0).on_click(|c| c.pop_top_layer()));
    });
}

fn open_confirm_restore(ctx: &mut EventContext) {
    open_confirm(
        ctx,
        "Restore this backup?",
        "This replaces your current vault and locks Knot. You'll unlock with the password that backup was made with.",
        "Restore",
        true,
    );
}

fn open_confirm_delete(ctx: &mut EventContext) {
    open_confirm(
        ctx,
        "Delete this backup?",
        "This backup file will be permanently deleted. This cannot be undone.",
        "Delete",
        true,
    );
}

// --- Row / field helpers -----------------------------------------------------

/// One backup row in the Restore list: `flex items-center justify-between gap-2
/// p-3 rounded-lg border hover:bg-…`, a filename + meta on the left and
/// Restore / Delete actions on the right.
fn backup_row(tree: &mut WidgetTree, parent: usize, b: &Backup) {
    let row = tree.add_child(
        parent,
        Container::row()
            .align_center()
            .justify(Justify::SpaceBetween)
            .gap(8.0)
            .padding(12.0) // p-3
            .radius(8.0)
            .border(1.0, section_border())
            .hoverable()
            .hover_background(tokens::pick(
                tokens::gray_50(),
                // dark:hover:bg-gray-700/50 — gray-700 at 50% alpha.
                Color::from_rgba8(0x37, 0x41, 0x51, 128),
            )),
    );

    let info = tree.add_child(row, Container::column().grow(1.0).gap(2.0));
    tree.add_child(
        info,
        TextWidget::new(b.file_name)
            .font_size(14.0)
            .weight(FontWeight::MEDIUM)
            .truncate(true)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        info,
        TextWidget::new(b.meta)
            .font_size(12.0)
            .color(tokens::muted()),
    );

    // Actions (`flex gap-1`): `text-xs px-2 py-1 rounded` buttons.
    let actions = tree.add_child(row, Container::row().gap(4.0));
    tree.add_child(
        actions,
        btn_primary("Restore", 4.0)
            .font_size(12.0)
            .on_click(open_confirm_restore),
    );
    tree.add_child(
        actions,
        btn_secondary("Delete", 4.0)
            .font_size(12.0)
            .on_click(open_confirm_delete),
    );
}

/// A labelled password field: `text-sm font-medium` label over a `px-3 py-2
/// border rounded-lg` [`SecureInput`]. Returns the group id so the caller can
/// append extra UI (the strength meter) beneath the input.
fn field(tree: &mut WidgetTree, parent: usize, label: &str, placeholder: &str) -> usize {
    let group = tree.add_child(parent, Container::column().gap(4.0)); // label mb-1
    tree.add_child(
        group,
        TextWidget::new(label)
            .font_size(14.0)
            .weight(FontWeight::MEDIUM)
            .color(label_fg()),
    );
    // px-3 py-2 (FW-17). Fill/border stay default (theme-tracking) — G12.
    tree.add_child(
        group,
        SecureInput::new()
            .placeholder(placeholder.to_string())
            .font_size(14.0)
            .radius(8.0)
            .padding_x(12.0)
            .padding_y(8.0),
    );
    group
}

/// One column of the interval/retention grid: a `text-xs` label over a
/// `px-2 py-1 rounded` numeric input, seeded with a sample value.
fn number_col(tree: &mut WidgetTree, parent: usize, label: &str, value: &str) {
    let col = tree.add_child(parent, Container::column().grow(1.0).gap(4.0));
    tree.add_child(
        col,
        TextWidget::new(label)
            .font_size(12.0)
            .color(tokens::muted()),
    );
    tree.add_child(
        col,
        Input::new()
            .numeric()
            .value(Signal::new(value.to_string()))
            .font_size(14.0)
            .radius(4.0)
            .padding_x(8.0)
            .padding_y(4.0),
    );
}

/// The password strength meter (`mt-1.5`): four `h-1 flex-1 rounded-full` bars
/// over a `text-xs` label. Rendered statically at level 2 ("Fair", yellow) —
/// the live component drives level/color off the typed password.
fn strength_meter(tree: &mut WidgetTree, parent: usize) {
    let wrap = tree.add_child(parent, Container::column().gap(2.0));
    let bars = tree.add_child(wrap, Container::row().gap(4.0));
    let empty = tokens::pick(tokens::gray_300(), tokens::gray_600());
    for i in 0..4 {
        let filled = i < 2; // level 2 of 4
        let color: Reactive<Color> = if filled {
            tokens::yellow_500().into()
        } else {
            empty.clone()
        };
        tree.add_child(
            bars,
            Container::row()
                .grow(1.0)
                .height(4.0)
                .radius(2.0)
                .background(color),
        );
    }
    tree.add_child(
        wrap,
        TextWidget::new("Fair")
            .font_size(12.0)
            .color(tokens::pick(tokens::yellow_600(), tokens::yellow_500())),
    );
}

// --- Shared chrome -----------------------------------------------------------

/// The modal card shell: `bg-white dark:bg-gray-800 rounded-lg` at a fixed
/// width (the `w-full max-w-*` never hits `w-full` at the clone's window size,
/// so the max-width *is* the width). The `shadow-xl` has no primitive (G18).
fn card_column(width: f32) -> Container {
    Container::column()
        .width(width)
        .background(card_bg())
        .radius(8.0)
}

/// A modal title: `text-lg font-semibold` (`mb-4` folded into the card gap).
fn title(tree: &mut WidgetTree, parent: usize, text: &str) {
    tree.add_child(
        parent,
        TextWidget::new(text)
            .font_size(18.0)
            .weight(FontWeight::SEMIBOLD)
            .color(tokens::on_surface()),
    );
}

/// A vertical spacer of `h` px — for the non-uniform `mb-2`/`mb-6` gaps the
/// confirm dialog needs that a single column `gap` can't express (G4-adjacent).
fn v_space(tree: &mut WidgetTree, parent: usize, h: f32) {
    tree.add_child(parent, Container::row().height(h));
}

fn btn_primary(label: &str, radius: f32) -> Button {
    Button::new(label)
        .radius(radius)
        .font_size(14.0)
        .background(tokens::primary())
        .hover_background(tokens::primary_hover())
        .press_background(tokens::primary_hover())
        .text_color(Color::WHITE)
}

fn btn_secondary(label: &str, radius: f32) -> Button {
    // bg-gray-200 dark:bg-gray-700, hover one step darker/lighter.
    Button::new(label)
        .radius(radius)
        .font_size(14.0)
        .background(tokens::pick(tokens::gray_200(), tokens::gray_700()))
        .hover_background(tokens::pick(tokens::gray_300(), tokens::gray_600()))
        .press_background(tokens::pick(tokens::gray_300(), tokens::gray_600()))
        .text_color(label_fg())
}

fn btn_danger(label: &str, radius: f32) -> Button {
    // bg-red-600 hover:bg-red-700 (same in both modes).
    Button::new(label)
        .radius(radius)
        .font_size(14.0)
        .background(tokens::red_600())
        .hover_background(tokens::red_700())
        .press_background(tokens::red_700())
        .text_color(Color::WHITE)
}

/// Modal card background (`bg-white dark:bg-gray-800`).
fn card_bg() -> Reactive<Color> {
    tokens::pick(tokens::white(), tokens::gray_800())
}

/// Section divider inside a modal (`border-gray-200 dark:border-gray-700`).
fn section_border() -> Reactive<Color> {
    tokens::pick(tokens::gray_200(), tokens::gray_700())
}

/// Form-field border (`border-gray-300 dark:border-gray-600`).
fn field_border() -> Reactive<Color> {
    tokens::pick(tokens::gray_300(), tokens::gray_600())
}

/// Field-label / secondary-button text (`text-gray-700 dark:text-gray-300`).
fn label_fg() -> Reactive<Color> {
    tokens::pick(tokens::gray_700(), tokens::gray_300())
}

// --- Dummy data --------------------------------------------------------------

/// A dummy backup entry for the Restore list.
struct Backup {
    file_name: &'static str,
    meta: &'static str,
}

/// Enough rows to overflow the `max-h-[80vh]` card and exercise the scroll.
const BACKUPS: &[Backup] = &[
    Backup {
        file_name: "knot-2026-04-27-0914.knotbak",
        meta: "Apr 27, 2026, 09:14 \u{00B7} 2.4 MB",
    },
    Backup {
        file_name: "knot-2026-04-26-0902.knotbak",
        meta: "Apr 26, 2026, 09:02 \u{00B7} 2.4 MB",
    },
    Backup {
        file_name: "knot-2026-04-25-0858.knotbak",
        meta: "Apr 25, 2026, 08:58 \u{00B7} 2.3 MB",
    },
    Backup {
        file_name: "knot-2026-04-24-0911.knotbak",
        meta: "Apr 24, 2026, 09:11 \u{00B7} 2.3 MB",
    },
    Backup {
        file_name: "knot-2026-04-23-0847.knotbak",
        meta: "Apr 23, 2026, 08:47 \u{00B7} 2.2 MB",
    },
    Backup {
        file_name: "knot-2026-04-22-0903.knotbak",
        meta: "Apr 22, 2026, 09:03 \u{00B7} 2.2 MB",
    },
    Backup {
        file_name: "knot-2026-04-21-0919.knotbak",
        meta: "Apr 21, 2026, 09:19 \u{00B7} 2.1 MB",
    },
    Backup {
        file_name: "knot-2026-04-20-0855.knotbak",
        meta: "Apr 20, 2026, 08:55 \u{00B7} 2.1 MB",
    },
    Backup {
        file_name: "knot-2026-04-19-0908.knotbak",
        meta: "Apr 19, 2026, 09:08 \u{00B7} 2.0 MB",
    },
];
