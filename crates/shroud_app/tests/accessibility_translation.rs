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

use accesskit::{Action, ActionData, ActionRequest, NodeId, TreeId};
use shroud_app::a11y::{action_from_request, snapshot_to_tree_update};
use shroud_core::{AccessAction, Point, Rect, Size};
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

/// The inbound mirror of the gate above: `accesskit` has actions that carry
/// text *into* a control, and the translation must refuse every one of them —
/// so no assistive technology can push characters into a field, masked or not.
#[test]
fn no_action_can_carry_text_into_the_tree() {
    fn request(action: Action, data: Option<ActionData>) -> ActionRequest {
        ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: NodeId(7),
            data,
        }
    }

    const PAYLOAD: &str = "text-an-AT-tried-to-inject";
    let text_bearing = [
        request(
            Action::SetValue,
            Some(ActionData::Value(PAYLOAD.into())), // the string form of SetValue
        ),
        request(
            Action::ReplaceSelectedText,
            Some(ActionData::Value(PAYLOAD.into())),
        ),
    ];
    for req in text_bearing {
        assert_eq!(
            action_from_request(&req),
            None,
            "{:?} carrying text must not translate to any action",
            req.action
        );
    }

    // Positive control: the numeric form of the *same* accesskit action does
    // translate, so the refusal above is about the text, not about SetValue
    // being unimplemented.
    assert_eq!(
        action_from_request(&request(
            Action::SetValue,
            Some(ActionData::NumericValue(0.5))
        )),
        Some((7, AccessAction::SetValue(0.5))),
        "a numeric SetValue is the one value channel an AT gets"
    );
}

#[test]
fn requests_translate_to_the_framework_vocabulary() {
    fn bare(action: Action, target: u64) -> ActionRequest {
        ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: NodeId(target),
            data: None,
        }
    }

    for (action, expected) in [
        (Action::Click, AccessAction::Click),
        (Action::Focus, AccessAction::Focus),
        (Action::Increment, AccessAction::Increment),
        (Action::Decrement, AccessAction::Decrement),
    ] {
        assert_eq!(
            action_from_request(&bare(action, 42)),
            Some((42, expected)),
            "{action:?} must reach the tree as {expected:?} against its target"
        );
    }

    // Actions we don't implement are dropped rather than guessed at — an AT is
    // free to ask for anything in accesskit's much wider set.
    for action in [
        Action::Blur,
        Action::ShowContextMenu,
        Action::ScrollIntoView,
        Action::Expand,
        Action::SetValue, // without the numeric data it carries nothing
    ] {
        assert_eq!(
            action_from_request(&bare(action, 42)),
            None,
            "{action:?} is not implemented and must not be invented"
        );
    }
}
