//! Translation layer: framework-native [`AccessSnapshot`] → `accesskit::TreeUpdate`.
//!
//! This is the single place the workspace links `accesskit`, mirroring how the
//! winit key/IME translation lives here rather than in the widget crates. Held
//! as a pure function ([`snapshot_to_tree_update`]) with no winit / adapter
//! state, so it is unit-testable without a window — the same convention as
//! `translate_ime` (see the `feedback_test_translation_layer` habit): the
//! platform→framework and framework→platform edges each get independent tests.
//!
//! Both directions cross here: [`snapshot_to_tree_update`] describes the tree
//! to the OS, and [`action_from_request`] turns what an assistive technology
//! asks for back into the framework's own [`AccessAction`] vocabulary.
//!
//! # Secret safety
//!
//! The snapshot is already secret-safe (a protected node's value is
//! suppressed), and this layer preserves that: it reads the value only through
//! [`AccessNode::exposed_value`](shroud_core::AccessNode::exposed_value), which
//! returns `None` for a protected node. A masked field is emitted as a
//! `PasswordInput` with a name but no value — never its characters. The app
//! crate's hard test drives a `SecureInput` through this function and asserts
//! the resulting `TreeUpdate` carries no trace of the plaintext.
//!
//! The inbound direction is the mirror image: `accesskit` can carry text into a
//! control (`Action::SetValue` with `ActionData::Value`, `ReplaceSelectedText`),
//! and [`action_from_request`] translates **none** of it. Only a numeric
//! `SetValue` survives, for range controls. So the a11y channel is read-only
//! with respect to text in both directions, and no AT can push characters into
//! a field — including a masked one.

use accesskit::{
    Action, ActionData, ActionRequest, Node, NodeId, Rect as AkRect, Role, Toggled, Tree, TreeId,
    TreeUpdate,
};
use shroud_core::{AccessAction, AccessRole};
use shroud_widgets::accessibility::AccessSnapshot;

/// Map a framework role to its `accesskit` counterpart. A 1:1 mapping — the
/// framework vocabulary was chosen to line up with `accesskit::Role`.
fn to_role(role: AccessRole) -> Role {
    match role {
        AccessRole::Window => Role::Window,
        AccessRole::Group => Role::GenericContainer,
        AccessRole::Label => Role::Label,
        AccessRole::Button => Role::Button,
        AccessRole::CheckBox => Role::CheckBox,
        AccessRole::RadioGroup => Role::RadioGroup,
        AccessRole::RadioButton => Role::RadioButton,
        AccessRole::Switch => Role::Switch,
        AccessRole::Slider => Role::Slider,
        AccessRole::TabList => Role::TabList,
        AccessRole::Tab => Role::Tab,
        AccessRole::MenuItem => Role::MenuItem,
        AccessRole::TextInput => Role::TextInput,
        AccessRole::PasswordInput => Role::PasswordInput,
        AccessRole::ScrollView => Role::ScrollView,
        AccessRole::Dialog => Role::Dialog,
    }
}

/// Build an `accesskit::TreeUpdate` describing the whole tree for this frame.
///
/// Always carries a full node set plus the `Tree` root and current focus —
/// accesskit diffs it internally, so re-sending the full tree each active
/// frame is correct and simplest.
pub fn snapshot_to_tree_update(snapshot: &AccessSnapshot) -> TreeUpdate {
    let mut nodes = Vec::with_capacity(snapshot.entries.len());

    for entry in &snapshot.entries {
        let info = &entry.node;
        let mut node = Node::new(to_role(info.role));

        if let Some(name) = &info.name {
            node.set_label(name.as_str());
        }

        // Secret-safe read: `exposed_value` is `None` for a protected node, so
        // a masked field never contributes a value here.
        if let Some(value) = info.exposed_value() {
            node.set_value(value);
        }

        if info.disabled {
            node.set_disabled();
        }

        if let Some(checked) = info.checked {
            node.set_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            });
        }

        if let Some(range) = info.numeric {
            node.set_numeric_value(range.now);
            node.set_min_numeric_value(range.min);
            node.set_max_numeric_value(range.max);
        }

        if let Some(selected) = info.selected {
            node.set_selected(selected);
        }

        // What the AT may ask of this node. An AT only offers the actions a
        // node advertises, so this list is the operable surface — kept in step
        // with what `Widget::accessibility_action` actually honours, and
        // withheld entirely from a disabled node (the widget refuses anyway;
        // this stops the AT from offering the action in the first place).
        if !info.disabled {
            if entry.focusable {
                node.add_action(Action::Focus);
            }
            if info.role.is_activatable() {
                node.add_action(Action::Click);
            }
            // Range controls: stepping and absolute set. Keyed off the numeric
            // state rather than the role, so only a node that actually reports
            // a range claims them.
            if info.numeric.is_some() {
                node.add_action(Action::Increment);
                node.add_action(Action::Decrement);
                node.add_action(Action::SetValue);
            }
        }

        if entry.modal {
            node.set_modal();
        }

        if !entry.children.is_empty() {
            let children: Vec<NodeId> = entry.children.iter().map(|&c| NodeId(c)).collect();
            node.set_children(children);
        }

        // Bounds in physical-pixel viewport coordinates (the space our layout
        // runs in). accesskit `Rect` is (x0, y0, x1, y1).
        let b = entry.bounds;
        node.set_bounds(AkRect::new(
            b.origin.x as f64,
            b.origin.y as f64,
            (b.origin.x + b.size.width) as f64,
            (b.origin.y + b.size.height) as f64,
        ));

        nodes.push((NodeId(entry.id), node));
    }

    TreeUpdate {
        nodes,
        tree: Some(Tree::new(NodeId(snapshot.root_id))),
        tree_id: TreeId::ROOT,
        focus: NodeId(snapshot.focus_id),
    }
}

/// Translate an `accesskit::ActionRequest` into the framework's own action
/// vocabulary, or `None` for anything we don't implement.
///
/// The inbound counterpart of [`snapshot_to_tree_update`], and the reason both
/// live in one pure function apiece: the translation is testable without a
/// window or an adapter. Returns the target node id alongside the action; the
/// tree resolves that id (see `WidgetTree::perform_access_action`).
///
/// An unknown action is `None` rather than an error — accesskit's `Action` set
/// is much wider than what a widget can honour, and an AT is free to ask.
/// Notably refused:
///
/// - **`SetValue` carrying text** (`ActionData::Value`) and
///   `ReplaceSelectedText`: no action may put characters into a widget. Only
///   `ActionData::NumericValue` maps through, for range controls.
/// - **`Blur`**: focus goes somewhere, it doesn't evaporate. An AT that wants
///   focus elsewhere sends `Focus` to that node.
pub fn action_from_request(request: &ActionRequest) -> Option<(u64, AccessAction)> {
    let action = match (request.action, &request.data) {
        (Action::Click, _) => AccessAction::Click,
        (Action::Focus, _) => AccessAction::Focus,
        (Action::Increment, _) => AccessAction::Increment,
        (Action::Decrement, _) => AccessAction::Decrement,
        (Action::SetValue, Some(ActionData::NumericValue(v))) => AccessAction::SetValue(*v),
        _ => return None,
    };
    Some((request.target_node.0, action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shroud_core::{AccessNode, AccessRange, AccessRole, Point, Rect, Size};
    use shroud_widgets::accessibility::{A11Y_WINDOW_ROOT, AccessEntry, AccessSnapshot};

    fn entry(id: u64, node: AccessNode) -> AccessEntry {
        AccessEntry {
            id,
            node,
            bounds: Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: Size {
                    width: 10.0,
                    height: 10.0,
                },
            },
            children: Vec::new(),
            modal: false,
            focusable: false,
        }
    }

    /// The single translated node for a one-entry snapshot, for asserting on
    /// what it advertises.
    fn translate_one(entry: AccessEntry) -> Node {
        let snap = AccessSnapshot {
            root_id: A11Y_WINDOW_ROOT,
            focus_id: A11Y_WINDOW_ROOT,
            entries: vec![entry],
        };
        snapshot_to_tree_update(&snap).nodes.remove(0).1
    }

    fn one_node_update(node: AccessNode) -> TreeUpdate {
        let snap = AccessSnapshot {
            root_id: A11Y_WINDOW_ROOT,
            focus_id: A11Y_WINDOW_ROOT,
            entries: vec![entry(1, node)],
        };
        snapshot_to_tree_update(&snap)
    }

    #[test]
    fn protected_node_translates_without_its_value() {
        // Independent of the widgets: hand a protected node whose value a caller
        // tried to set, and confirm the translated accesskit tree drops it.
        const SECRET: &str = "leak-me-if-you-can";
        let node = AccessNode::new(AccessRole::PasswordInput)
            .name("Password")
            .protected()
            .value(SECRET); // refused by the guarded setter
        let dump = format!("{:?}", one_node_update(node));
        assert!(
            !dump.contains(SECRET),
            "translation must not carry the secret"
        );
        assert!(dump.contains("Password"), "the non-secret name still maps");
    }

    #[test]
    fn role_name_and_toggle_map_through() {
        let node = AccessNode::new(AccessRole::CheckBox)
            .name("Agree")
            .checked(true);
        let update = one_node_update(node);
        assert_eq!(update.nodes.len(), 1, "one widget → one node");
        let dump = format!("{update:?}");
        assert!(dump.contains("CheckBox"), "role maps to accesskit CheckBox");
        assert!(dump.contains("Agree"), "name maps to the accesskit label");
    }

    #[test]
    fn a_control_advertises_the_actions_it_honours() {
        let mut e = entry(1, AccessNode::new(AccessRole::Button).name("Save"));
        e.focusable = true;
        let node = translate_one(e);
        assert!(node.supports_action(Action::Click), "a button is pressable");
        assert!(node.supports_action(Action::Focus), "and focusable");
        assert!(
            !node.supports_action(Action::SetValue),
            "a button has no value to set"
        );
    }

    #[test]
    fn a_range_control_advertises_stepping_and_set() {
        let mut e = entry(
            1,
            AccessNode::new(AccessRole::Slider).numeric(AccessRange {
                min: 0.0,
                max: 10.0,
                now: 5.0,
            }),
        );
        e.focusable = true;
        let node = translate_one(e);
        for action in [Action::Increment, Action::Decrement, Action::SetValue] {
            assert!(node.supports_action(action), "a slider honours {action:?}");
        }
        assert!(
            !node.supports_action(Action::Click),
            "a slider is not pressable — Slider is not an activatable role"
        );
    }

    #[test]
    fn a_disabled_control_advertises_nothing() {
        // The widget refuses the action anyway; withholding it here stops the
        // AT from offering it in the first place.
        let mut e = entry(
            1,
            AccessNode::new(AccessRole::Button)
                .name("Save")
                .disabled(true),
        );
        e.focusable = true;
        let node = translate_one(e);
        for action in [Action::Click, Action::Focus, Action::SetValue] {
            assert!(
                !node.supports_action(action),
                "a disabled node must not offer {action:?}"
            );
        }
    }

    #[test]
    fn a_secret_field_only_offers_focus() {
        // A masked field is perceivable (role + name) and focusable, and that
        // is the whole operable surface it gets: nothing that reads or writes
        // its characters.
        let mut e = entry(
            1,
            AccessNode::new(AccessRole::PasswordInput)
                .name("Password")
                .protected(),
        );
        e.focusable = true;
        let node = translate_one(e);
        assert!(node.supports_action(Action::Focus));
        for action in [
            Action::Click,
            Action::SetValue,
            Action::ReplaceSelectedText,
            Action::Increment,
        ] {
            assert!(
                !node.supports_action(action),
                "a protected node must not offer {action:?}"
            );
        }
    }

    #[test]
    fn selected_state_maps_through_for_options() {
        let node = translate_one(entry(
            1,
            AccessNode::new(AccessRole::Tab)
                .name("Preview")
                .selected(true),
        ));
        assert_eq!(
            node.is_selected(),
            Some(true),
            "an option reports its selected state"
        );
    }

    #[test]
    fn focus_id_maps_to_node_id() {
        let snap = AccessSnapshot {
            root_id: A11Y_WINDOW_ROOT,
            focus_id: 7,
            entries: vec![entry(7, AccessNode::new(AccessRole::Button).name("Go"))],
        };
        let update = snapshot_to_tree_update(&snap);
        assert_eq!(update.focus, NodeId(7));
    }
}
