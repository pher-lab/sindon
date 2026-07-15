//! Translation layer: framework-native [`AccessSnapshot`] → `accesskit::TreeUpdate`.
//!
//! This is the single place the workspace links `accesskit`, mirroring how the
//! winit key/IME translation lives here rather than in the widget crates. Held
//! as a pure function ([`snapshot_to_tree_update`]) with no winit / adapter
//! state, so it is unit-testable without a window — the same convention as
//! `translate_ime` (see the `feedback_test_translation_layer` habit): the
//! platform→framework and framework→platform edges each get independent tests.
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

use accesskit::{Node, NodeId, Rect as AkRect, Role, Toggled, Tree, TreeId, TreeUpdate};
use shroud_core::AccessRole;
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
        AccessRole::RadioButton => Role::RadioButton,
        AccessRole::Switch => Role::Switch,
        AccessRole::Slider => Role::Slider,
        AccessRole::TabList => Role::TabList,
        AccessRole::Tab => Role::Tab,
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

        // NOTE: per-option `selected` state is not emitted yet — no MVP widget
        // populates it (Segmented / RadioGroup are single nodes for now). It
        // arrives with the per-option child nodes in the operable slice.

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

#[cfg(test)]
mod tests {
    use super::*;
    use shroud_core::{AccessNode, AccessRole, Point, Rect, Size};
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
        }
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
