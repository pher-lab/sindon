//! Knot UI clone — a UI-only reproduction of the React/Tailwind Knot app,
//! built to surface where sindon's visual vocabulary falls short.
//!
//! See `docs/knot-ui-repro-gaps.md` for the running gap log. No crypto / DB:
//! screens use dummy data and navigate by `replace_screen`. Ctrl+D toggles
//! dark mode so both palettes can be captured from one binary.

mod loading;
mod main_screen;
mod modals;
mod recovery;
mod setup;
mod tokens;
mod unlock;

use sindon::app::App;
use sindon::reactive::Reactive;
use sindon::widgets::tree::WidgetTree;
use sindon::widgets::{Key, Modifiers, Shortcut};

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

            // Dev-only nav to jump between the sibling screens for side-by-side
            // review — the real app picks one from vault state (loading → setup
            // or unlock), so these links don't exist there.
            // Ctrl+1 = Loading, Ctrl+2 = Setup, Ctrl+3 = Unlock, Ctrl+4 = Recovery.
            scope.on_shortcut(
                Shortcut::global(Modifiers::CTRL, Key::Character('1')),
                |ctx| ctx.event_ctx.replace_screen(loading::build),
            );
            scope.on_shortcut(
                Shortcut::global(Modifiers::CTRL, Key::Character('2')),
                |ctx| ctx.event_ctx.replace_screen(setup::build),
            );
            scope.on_shortcut(
                Shortcut::global(Modifiers::CTRL, Key::Character('3')),
                |ctx| ctx.event_ctx.replace_screen(unlock::build),
            );
            scope.on_shortcut(
                Shortcut::global(Modifiers::CTRL, Key::Character('4')),
                |ctx| ctx.event_ctx.replace_screen(recovery::build),
            );

            let mut tree = WidgetTree::new();
            unlock::build(&mut tree);
            tree
        });
}
