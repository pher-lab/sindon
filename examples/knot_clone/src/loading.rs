//! Loading screen — faithful reproduction of `App.tsx`'s inline
//! `LoadingScreen` (shown while the app checks vault state before routing to
//! setup / unlock).
//!
//! Reference layout (Tailwind) — the simplest screen in the app, and notably
//! there is **no spinner**, just centered text:
//!   - Full-screen page bg, flex center both axes. Unlike the other screens
//!     there is *no* `p-4` padding.
//!   - A `text-center` block: "Knot" `text-4xl font-bold mb-4`, then a
//!     `text-gray-500` "Loading..." line (default `text-base`).
//!
//! Nothing new to exercise: this is `background()` + two `TextWidget`s in a
//! centered column — no primitive we haven't used since the unlock screen, and
//! **no new gap**. With this, every screen of the app has been reproduced.
//!
//! UI-only clone: purely static (no vault check to run). Reached via the
//! dev-nav shortcut in `main.rs` (Ctrl+1).

use sindon::text::FontWeight;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Container, TextWidget};

use crate::tokens;

pub fn build(tree: &mut WidgetTree) {
    // Outer: page background, centered both axes. No `p-4` here (the reference
    // omits it), and `align_center` centers the text block horizontally.
    let root = tree.set_root(
        Container::column()
            .width_full()
            .height_full()
            .background(tokens::background())
            .justify_center()
            .align_center(),
    );

    // `text-center` block: "Knot" + "Loading...". The `mb-4` (16) between the
    // heading and the caption is the column gap.
    let content = tree.add_child(root, Container::column().align_center().gap(16.0));
    tree.add_child(
        content,
        TextWidget::new("Knot")
            .font_size(36.0) // text-4xl
            .weight(FontWeight::BOLD)
            .color(tokens::on_surface()),
    );
    tree.add_child(
        content,
        TextWidget::new("Loading...") // app.loading
            .font_size(16.0) // text-base
            .color(tokens::muted()),
    );
}
