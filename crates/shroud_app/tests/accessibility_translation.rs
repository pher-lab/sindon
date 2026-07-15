//! App-layer secret-safety gate: the framework→`accesskit` translation must
//! never carry a secret into the tree it hands the OS.
//!
//! This is the second, independent half of the two-layer guard
//! (`feedback_test_translation_layer`): the widget crate asserts the *snapshot*
//! omits secrets; here we drive the full chain — widget → snapshot →
//! `accesskit::TreeUpdate` — and scan the final translated output. The scan is
//! getter-agnostic on purpose: it searches the `Debug` dump of the whole
//! `TreeUpdate`, so any label / value / description that leaked a secret would
//! trip it, regardless of which accesskit field it landed in. In the residue
//! spirit, a positive control proves the pipeline actually carries text so the
//! negative assertion can't pass vacuously.

use shroud_app::a11y::snapshot_to_tree_update;
use shroud_core::{Point, Rect, Size};
use shroud_security::SecureString;
use shroud_widgets::*;

const RECT: Rect = Rect {
    origin: Point { x: 0.0, y: 0.0 },
    size: Size {
        width: 200.0,
        height: 40.0,
    },
};

fn typed_secure_input(text: &str) -> SecureInput {
    let mut si = SecureInput::new().placeholder("Enter password");
    let mut ev = EventContext::new();
    si.event(&WidgetEvent::FocusGained, RECT, &mut ev);
    for ch in text.chars() {
        si.event(&WidgetEvent::CharInput { ch }, RECT, &mut ev);
    }
    si
}

fn typed_input(text: &str) -> Input {
    let mut input = Input::new();
    let mut ev = EventContext::new();
    input.event(&WidgetEvent::FocusGained, RECT, &mut ev);
    for ch in text.chars() {
        input.event(&WidgetEvent::CharInput { ch }, RECT, &mut ev);
    }
    input
}

#[test]
fn translated_tree_update_never_carries_a_secret() {
    const SECRET: &str = "s3cr3t-passphrase-do-not-leak";
    const NOTE: &str = "visible-note-body-text";

    let mut tree = WidgetTree::new();
    let root = tree.set_root(Container::column().width(400.0).height(300.0));
    tree.add_child(root, typed_secure_input(SECRET)); // masked entry
    tree.add_child(root, SecureText::new(SecureString::new(SECRET))); // masked display
    tree.add_child(root, typed_input(NOTE)); // ordinary content (positive control)
    tree.compute_layout(800.0, 600.0);

    let snapshot = tree.accessibility_snapshot();
    let update = snapshot_to_tree_update(&snapshot);
    let dump = format!("{update:?}");

    assert!(
        !dump.contains(SECRET),
        "the translated accesskit TreeUpdate must not contain the secret anywhere"
    );
    assert!(
        dump.contains(NOTE),
        "a plain input's text must reach the accesskit tree — otherwise the \
         no-secret assertion above would pass vacuously"
    );
}
