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
//! [`AccessNode::exposed_value`](sindon_core::AccessNode::exposed_value), which
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
use sindon_core::{AccessAction, AccessRole};
use sindon_widgets::accessibility::AccessSnapshot;

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
        AccessRole::ProgressIndicator => Role::ProgressIndicator,
        AccessRole::TabList => Role::TabList,
        AccessRole::Tab => Role::Tab,
        AccessRole::Tree => Role::Tree,
        AccessRole::TreeItem => Role::TreeItem,
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
pub fn snapshot_to_tree_update(snapshot: &AccessSnapshot, scale: f32) -> TreeUpdate {
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

        // Hierarchy, for rows flattened into one list: the depth is what tells a
        // screen reader "level 3" where the indentation tells a sighted user.
        if let Some(level) = info.level {
            node.set_level(level);
        }

        if let Some(expanded) = info.expanded {
            node.set_expanded(expanded);
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
            // Range controls: stepping and absolute set. A node must both report
            // a range *and* have an operable role — a `ProgressIndicator` reports
            // a numeric value but is read-only, so it advertises none of these.
            if info.numeric.is_some() && info.role.is_value_adjustable() {
                node.add_action(Action::Increment);
                node.add_action(Action::Decrement);
                node.add_action(Action::SetValue);
            }
            // Disclosure: only the direction the node isn't already in, and only
            // for a branch — a leaf reports no `expanded` state at all, so it
            // offers neither.
            if info.role.is_expandable() {
                match info.expanded {
                    Some(true) => node.add_action(Action::Collapse),
                    Some(false) => node.add_action(Action::Expand),
                    None => {}
                }
            }
        }

        if entry.modal {
            node.set_modal();
        }

        if !entry.children.is_empty() {
            let children: Vec<NodeId> = entry.children.iter().map(|&c| NodeId(c)).collect();
            node.set_children(children);
        }

        // accesskit wants physical-pixel bounds: the platform adapter adds the
        // window's client origin (also physical) and hands the result straight
        // to the screen reader, applying no scale of its own. Layout runs in
        // logical pixels, so this is the one place that converts. The multiply
        // was absent while layout was itself physical — it was an identity,
        // not an exemption.
        // accesskit `Rect` is (x0, y0, x1, y1).
        let b = entry.bounds;
        let (x0, y0) = (b.origin.x * scale, b.origin.y * scale);
        node.set_bounds(AkRect::new(
            x0 as f64,
            y0 as f64,
            (x0 + b.size.width * scale) as f64,
            (y0 + b.size.height * scale) as f64,
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
        (Action::Expand, _) => AccessAction::Expand,
        (Action::Collapse, _) => AccessAction::Collapse,
        _ => return None,
    };
    Some((request.target_node.0, action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sindon_core::{AccessNode, AccessRange, AccessRole, Point, Rect, Size};
    use sindon_widgets::accessibility::{A11Y_WINDOW_ROOT, AccessEntry, AccessSnapshot};

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
        snapshot_to_tree_update(&snap, 1.0).nodes.remove(0).1
    }

    fn one_node_update(node: AccessNode) -> TreeUpdate {
        let snap = AccessSnapshot {
            root_id: A11Y_WINDOW_ROOT,
            focus_id: A11Y_WINDOW_ROOT,
            entries: vec![entry(1, node)],
        };
        snapshot_to_tree_update(&snap, 1.0)
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
    fn a_progress_indicator_reports_a_value_but_is_not_operable() {
        // A determinate progress bar reports where it is (min/max/now) so a
        // screen reader can announce the percentage, but it is read-only: unlike
        // a Slider it must advertise none of the value-setting actions, since its
        // widget honours none.
        let mut e = entry(
            1,
            AccessNode::new(AccessRole::ProgressIndicator)
                .name("Uploading")
                .numeric(AccessRange {
                    min: 0.0,
                    max: 1.0,
                    now: 0.6,
                }),
        );
        e.focusable = false;
        let node = translate_one(e);
        let dump = format!("{node:?}");
        assert!(
            dump.contains("ProgressIndicator"),
            "role maps to accesskit ProgressIndicator"
        );
        assert!(dump.contains("Uploading"), "the name maps through");
        for action in [Action::Increment, Action::Decrement, Action::SetValue] {
            assert!(
                !node.supports_action(action),
                "a progress indicator must not offer {action:?} — it is read-only"
            );
        }
        assert!(
            !node.supports_action(Action::Click),
            "a progress indicator is not activatable"
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
    fn a_tree_row_carries_its_depth_and_disclosure() {
        // The rows reach the AT as one flat list, so `level` is the whole
        // hierarchy as far as a screen reader is concerned.
        let node = translate_one(entry(
            1,
            AccessNode::new(AccessRole::TreeItem)
                .name("widgets")
                .level(2)
                .expanded(true),
        ));
        let dump = format!("{node:?}");
        assert!(dump.contains("TreeItem"), "role maps to accesskit TreeItem");
        assert_eq!(node.level(), Some(2));
        assert_eq!(node.is_expanded(), Some(true));
    }

    #[test]
    fn a_branch_only_offers_the_disclosure_it_is_not_already_in() {
        let closed = translate_one(entry(
            1,
            AccessNode::new(AccessRole::TreeItem)
                .name("src")
                .expanded(false),
        ));
        assert!(closed.supports_action(Action::Expand), "a closed row opens");
        assert!(
            !closed.supports_action(Action::Collapse),
            "and must not offer to close what is already closed"
        );

        let open = translate_one(entry(
            1,
            AccessNode::new(AccessRole::TreeItem)
                .name("src")
                .expanded(true),
        ));
        assert!(open.supports_action(Action::Collapse));
        assert!(!open.supports_action(Action::Expand));
    }

    #[test]
    fn a_leaf_row_is_clickable_but_offers_no_disclosure() {
        // A leaf reports no `expanded` state at all, which is what keeps the
        // disclosure actions off it — while it stays selectable like any item.
        let leaf = translate_one(entry(
            1,
            AccessNode::new(AccessRole::TreeItem).name("main.rs"),
        ));
        assert!(leaf.supports_action(Action::Click), "an item is selectable");
        for action in [Action::Expand, Action::Collapse] {
            assert!(
                !leaf.supports_action(action),
                "a leaf must not offer {action:?} — it has nothing to disclose"
            );
        }
    }

    #[test]
    fn a_non_tree_node_never_advertises_a_disclosure() {
        // The gate is the role, not the state: nothing else can be talked into
        // offering Expand by carrying an `expanded` flag.
        let node = translate_one(entry(
            1,
            AccessNode::new(AccessRole::Button)
                .name("Save")
                .expanded(false),
        ));
        assert!(!node.supports_action(Action::Expand));
    }

    #[test]
    fn focus_id_maps_to_node_id() {
        let snap = AccessSnapshot {
            root_id: A11Y_WINDOW_ROOT,
            focus_id: 7,
            entries: vec![entry(7, AccessNode::new(AccessRole::Button).name("Go"))],
        };
        let update = snapshot_to_tree_update(&snap, 1.0);
        assert_eq!(update.focus, NodeId(7));
    }

    #[test]
    fn disclosure_requests_translate_inbound() {
        // The other direction of the same pair — the AT asking, rather than
        // being told what it may ask for.
        for (ak, expected) in [
            (Action::Expand, AccessAction::Expand),
            (Action::Collapse, AccessAction::Collapse),
        ] {
            let request = ActionRequest {
                action: ak,
                target_tree: TreeId::ROOT,
                target_node: NodeId(12),
                data: None,
            };
            assert_eq!(action_from_request(&request), Some((12, expected)));
        }
    }

    #[test]
    fn bounds_convert_from_logical_to_physical() {
        // The AT is handed physical pixels: the platform adapter adds the
        // window's client origin and applies no scale of its own. A widget laid
        // out at logical (10, 20) sized 100x40 must therefore reach accesskit as
        // (20, 40)-(220, 120) on a 200% display. The multiply reads as a no-op
        // at 100%, which is exactly how it stayed missing for as long as layout
        // was itself physical — so pin it at a scale where it isn't one.
        let mut e = entry(1, AccessNode::new(AccessRole::Button).name("Go"));
        e.bounds = Rect {
            origin: Point { x: 10.0, y: 20.0 },
            size: Size {
                width: 100.0,
                height: 40.0,
            },
        };
        let snap = AccessSnapshot {
            root_id: A11Y_WINDOW_ROOT,
            focus_id: A11Y_WINDOW_ROOT,
            entries: vec![e],
        };

        let update = snapshot_to_tree_update(&snap, 2.0);

        assert_eq!(
            update.nodes[0].1.bounds(),
            Some(AkRect::new(20.0, 40.0, 220.0, 120.0)),
            "logical bounds must be scaled into physical pixels for the AT"
        );
    }
}
