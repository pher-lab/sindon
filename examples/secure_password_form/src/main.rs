//! Secure Password Form — flagship shroud example.
//!
//! Demonstrates the full security pipeline:
//! - SecureInput: characters go directly into SecureString (no String intermediary)
//! - Masked display: actual text is never rendered, only characters
//! - Secure atlas: sensitive glyphs cleared from GPU every frame
//! - Screen capture prevention: window appears black in screenshots (Windows)
//! - Zeroize on drop: all sensitive data zeroized when the app exits
//!
//! Try it:
//! 1. Click the password field
//! 2. Type a password — you'll see masked chars
//! 3. Click "Login" or press Enter
//! 4. Take a screenshot — the window should appear black (Windows)

use shroud::app::App;
use shroud::core::Color;
use shroud::widgets::tree::WidgetTree;
use shroud::widgets::{Button, Container, SecureInput, TextWidget};

fn main() {
    App::new()
        .title("shroud \u{2014} Secure Password Form")
        .size(600, 500)
        .capture_prevention(true)
        .run(|_handle| {
            let mut tree = WidgetTree::new();

            // Root: centered column layout (background comes from theme)
            let root = tree.set_root(Container::column().width_full().height_full().center());

            // Form container: fixed width, surface background from theme
            let form = tree.add_child(
                root,
                Container::column().width(360.0).padding(32.0).gap(16.0),
            );

            // Title — uses primary accent color
            tree.add_child(
                form,
                TextWidget::new("shroud")
                    .font_size(28.0)
                    .color(Color::rgb(0.4, 0.5, 0.9)),
            );

            // Subtitle — muted text, uses theme default
            tree.add_child(form, TextWidget::new("Secure Login"));

            // Username label
            tree.add_child(form, TextWidget::new("Username"));

            // Username input (plain text — not sensitive)
            tree.add_child(form, TextWidget::new("admin@example.com"));

            // Password label
            tree.add_child(form, TextWidget::new("Password"));

            // Password input (SecureInput — the star of the show)
            tree.add_child(form, SecureInput::new().placeholder("Enter password"));

            // Login button — theme primary colors
            tree.add_child(form, Button::new("Login"));

            // Security notice
            tree.add_child(
                form,
                TextWidget::new("Protected by shroud \u{2014} zeroize on drop"),
            );

            tree
        });
}
