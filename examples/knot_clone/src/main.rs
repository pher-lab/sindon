//! Knot UI clone — a UI-only reproduction of the React/Tailwind Knot app,
//! built to surface where shroud's visual vocabulary falls short.
//!
//! See `docs/knot-ui-repro-gaps.md` for the running gap log. No crypto / DB:
//! screens use dummy data and navigate by `replace_screen`. Ctrl+D toggles
//! dark mode so both palettes can be captured from one binary.

mod tokens;
mod unlock;

use shroud::app::App;
use shroud::reactive::Reactive;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Key, Modifiers, Shortcut};

fn main() {
    App::new()
        .title("Knot — UI clone")
        .size(1080, 720)
        .theme(Reactive::derive(tokens::theme))
        .run(|scope| {
            // Ctrl+D flips light/dark so we can screenshot both palettes.
            scope.on_shortcut(
                Shortcut::global(Modifiers::CTRL, Key::Character('d')),
                |_ctx| tokens::toggle_dark(),
            );

            let mut tree = WidgetTree::new();
            unlock::build(&mut tree);
            tree
        });
}
