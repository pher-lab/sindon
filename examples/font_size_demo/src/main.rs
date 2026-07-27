//! font_size_demo — Phase 31: UI-wide font scaling via `Theme::with_font_scale`.
//!
//! Demonstrates the pattern Knot's Sidebar "Font size" setting needs:
//! three presets (Small / Medium / Large) drive a `Signal<f32>`, which
//! a `Reactive::derive` folds into a scaled theme. The event loop pulls
//! the theme on every paint (Phase 30), so widgets that read from
//! `theme.typography` resize in lockstep — no per-Text rewiring.
//!
//! Text widgets without an explicit `font_size(px)` override pick up the
//! body size from the theme. The static override is still useful for
//! callouts that must stay fixed regardless of the user-chosen scale.

use sindon::app::App;
use sindon::core::Theme;
use sindon::reactive::{Reactive, Signal};
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Button, Container, TextWidget};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontChoice {
    Small,
    Medium,
    Large,
}

impl FontChoice {
    fn scale(self) -> f32 {
        // Knot's small/medium/large = 13/15/18 px relative to a 15 px
        // "medium" baseline → 0.867 / 1.0 / 1.2. Rounded for readability.
        match self {
            FontChoice::Small => 0.85,
            FontChoice::Medium => 1.0,
            FontChoice::Large => 1.25,
        }
    }
}

fn main() {
    let choice: Signal<FontChoice> = Signal::new(FontChoice::Medium);

    let theme = Reactive::derive(move || Theme::dark().with_font_scale(choice.get().scale()));

    App::new()
        .title("sindon \u{2014} font size demo (Phase 31)")
        .size(640, 420)
        .theme(theme)
        .run(move |_scope| {
            let mut tree = WidgetTree::new();
            let root = tree.set_root(
                Container::column()
                    .width_full()
                    .height_full()
                    .padding(24.0)
                    .gap(16.0),
            );

            // Title — TextWidget::new defaults to theme.typography.body.
            // When the global font scale changes, this line resizes along
            // with everything else that reads the body token.
            tree.add_child(root, TextWidget::new("Font size demo"));

            // Body-sized paragraph that reflows on every preset switch.
            tree.add_child(
                root,
                TextWidget::new(
                    "This paragraph also reads body-sized text from the \
                     theme. Pick a preset below and both lines resize \
                     in lockstep without any code touching the Text \
                     widgets themselves.",
                ),
            );

            tree.add_child(
                root,
                TextWidget::reactive(move || {
                    let (label, scale) = match choice.get() {
                        FontChoice::Small => ("Small", 0.85_f32),
                        FontChoice::Medium => ("Medium", 1.0),
                        FontChoice::Large => ("Large", 1.25),
                    };
                    format!("Current: {label} ({scale:.2}\u{00d7})")
                }),
            );

            let buttons = tree.add_child(root, Container::row().gap(12.0));
            for (label, c) in [
                ("Small", FontChoice::Small),
                ("Medium", FontChoice::Medium),
                ("Large", FontChoice::Large),
            ] {
                tree.add_child(
                    buttons,
                    Button::new(label).radius(6.0).on_click(move |_ctx| {
                        choice.set(c);
                    }),
                );
            }

            // Pinned-size note — `font_size` and `line_height` are
            // independent Options on TextWidget; both must be overridden
            // for the row to stay fully fixed. Pinning `font_size` alone
            // keeps the *glyphs* at 11 px but lets the row's line height
            // inherit (and therefore scale with) the theme's body token.
            tree.add_child(
                root,
                TextWidget::new(
                    "(This footnote pins both font_size(11.0) and \
                     line_height(15.0), so it stays fully fixed while the \
                     rest of the UI scales.)",
                )
                .font_size(11.0)
                .line_height(15.0),
            );

            tree
        });
}
